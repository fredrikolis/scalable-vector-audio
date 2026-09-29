// Concern: the one table of values, keyed by identity and step, that renders, streams, prices and logs read | Non-concern: typing, readings off samples | IO: (Typing, roots) -> Table, samples

mod demand;
pub(crate) mod edit;
mod eval;
#[cfg(test)]
mod laws;
mod period;
pub(crate) mod program;
mod segments;
mod store;
pub(crate) mod support;
mod value;

use std::collections::{BTreeMap, HashMap};

use sva_formula::{ClosedForm, Hash, Held as Representation, NodeId, Var};
use sva_samples::{
    Buffer, Extent, Formula, Grid, Label, NodeRenderer, Profile, Rows, Slot, Spanned, Tape,
    truncate_spectral_sum, truncate_written,
};

pub(crate) use demand::Need;
pub(crate) use value::{Held, Key, Kind, Value};

use crate::cache::Recording;
use crate::cast::Cast;
use crate::error::EngineError;
use crate::refs;
use crate::time::Lattice;
use crate::typing::{Typing, Value as Typed};
use program::Source;
use support::Supports;
use value::Program;

pub(crate) struct Table {
    pub(crate) values: Vec<Value>,
    nodes: BTreeMap<NodeId, usize>,
    pub(crate) root: usize,
    /// Each node a reading holds over the range, in its own time.
    pub(crate) wanted: Vec<usize>,
    /// What each value costs over the whole range, as a pull pays it.
    pub(crate) planned: Vec<u128>,
    places: Vec<store::Place>,
    profile: Profile,
    /// The most seconds any read was moved to land on a whole sample: half a sample at most.
    pub(crate) moved: f64,
}

impl Table {
    /// Every value `root` and `wanted` read, each once per identity and step, readers after
    /// the values they read.
    pub(crate) fn build(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
    ) -> Result<Table, EngineError> {
        Table::built(tys, (root, wanted), profile, 1, false)
    }

    /// Every read its own value, as if each were written out where it is read.
    #[cfg(test)]
    pub(crate) fn apart(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
    ) -> Result<Table, EngineError> {
        Table::built(tys, (root, wanted), profile, 1, true)
    }

    /// Every value `fine` times finer than its own step: a closed form's reference.
    pub(crate) fn finer(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
        fine: i128,
    ) -> Result<Table, EngineError> {
        Table::built(tys, (root, wanted), profile, fine, false)
    }

    fn built(
        tys: &Typing,
        (root, wanted): (NodeId, &[NodeId]),
        profile: &Profile,
        fine: i128,
        apart: bool,
    ) -> Result<Table, EngineError> {
        let supports = Supports::new(tys);
        let mut building = Building {
            tys,
            supports: &supports,
            profile,
            fine,
            apart,
            copies: 0,
            reading: Vec::new(),
            named: BTreeMap::new(),
            keys: HashMap::new(),
            open: Vec::new(),
            values: Vec::new(),
            nodes: BTreeMap::new(),
            moved: 0.0,
        };
        let root = building.node(root)?;
        let wanted = wanted
            .iter()
            .map(|id| building.node(*id))
            .collect::<Result<Vec<_>, _>>()?;
        let target = aliased(&building.values, root);
        let places = places(&building.values, target, profile);
        Ok(Table {
            values: building.values,
            nodes: building.nodes,
            root,
            wanted,
            planned: Vec::new(),
            places,
            profile: *profile,
            moved: building.moved,
        })
    }

    /// Each value a volatile parameter reaches keeps one entry, its last, under its slot.
    pub(crate) fn slots(&mut self, slot: impl Fn(NodeId) -> Option<Hash>) {
        for (value, place) in self.values.iter().zip(&mut self.places) {
            place.slot = value.node.and_then(&slot);
        }
    }

    /// Prices every value over `range` before a sample is computed.
    pub(crate) fn plan(&mut self, range: Extent) {
        let needs = self.demand(range);
        self.planned = self.price(&needs);
    }

    /// A table of nothing, standing in while its successor takes over.
    pub(crate) fn empty() -> Table {
        Table {
            values: Vec::new(),
            nodes: BTreeMap::new(),
            root: 0,
            wanted: Vec::new(),
            planned: Vec::new(),
            places: Vec::new(),
            profile: sva_samples::PSYCHOACOUSTIC_V1,
            moved: 0.0,
        }
    }

    pub(crate) fn of(&self, node: NodeId) -> Option<usize> {
        self.nodes.get(&node).copied()
    }

    /// What a window of the root, and of each wanted node, asks of every value.
    pub(crate) fn demand(&self, window: Extent) -> Vec<Need> {
        demand::demand(&self.values, &self.asked(window))
    }

    fn asked(&self, window: Extent) -> Vec<(usize, Extent)> {
        std::iter::once(self.root)
            .chain(self.wanted.iter().copied())
            .map(|v| (v, window))
            .collect()
    }

    /// Computes what `window` asks that is not held, each value once, after the store answers
    /// what it holds; the price of what ran.
    pub(crate) fn pull(
        &mut self,
        window: Extent,
        recording: &mut Recording,
    ) -> Result<Pulled, EngineError> {
        self.pulled(&self.asked(window), recording)
    }

    /// The history each stateful value runs through before what `window` holds of it, pulled
    /// `block` samples at a time, each block dropped once no later one reads it: the state a
    /// late window starts from, streamed as a render from its start streams it.
    pub(crate) fn history(
        &mut self,
        window: Extent,
        block: i64,
        recording: &mut Recording,
    ) -> Result<Pulled, EngineError> {
        let spans: Vec<(usize, Extent)> = self
            .demand(window)
            .iter()
            .enumerate()
            .filter(|(at, _)| matches!(&self.values[*at].kind, Kind::Program(p) if p.stateful()))
            .filter_map(|(at, need)| {
                let span = Extent::new(need.compute.hull().start, need.hold.hull().start);
                (!need.compute.is_empty() && !span.is_empty()).then_some((at, span))
            })
            .collect();
        let over = spans
            .iter()
            .fold(Extent::NOWHERE, |held, (_, s)| held.hull(*s));
        let within = |cut: Extent| -> Vec<(usize, Extent)> {
            spans
                .iter()
                .map(|(at, span)| (*at, span.intersect(cut)))
                .filter(|(_, span)| !span.is_empty())
                .collect()
        };
        let mut pulled = Pulled::default();
        let mut from = over.start;
        while from < over.end {
            let to = from.saturating_add(block).min(over.end);
            let done = self.pulled(&within(Extent::new(from, to)), recording)?;
            pulled.priced += done.priced;
            pulled.waves += done.waves;
            pulled.most_bytes = pulled.most_bytes.max(done.most_bytes);
            let mut later = within(Extent::new(to, i64::MAX));
            later.extend(self.asked(window));
            self.released(
                demand::demand(&self.values, &later),
                Extent::NOWHERE,
                window.start,
            );
            from = to;
        }
        Ok(pulled)
    }

    fn pulled(
        &mut self,
        asked: &[(usize, Extent)],
        recording: &mut Recording,
    ) -> Result<Pulled, EngineError> {
        let mut needs = demand::demand(&self.values, asked);
        loop {
            let mut loaded = false;
            for (at, need) in needs.iter().enumerate() {
                let place = &mut self.places[at];
                if need.hold.is_empty() || place.looked || !self.values[at].pure {
                    continue;
                }
                loaded |= store::load(&mut self.values[at], place, recording);
            }
            if !loaded {
                break;
            }
            needs = demand::demand(&self.values, asked);
        }
        let mut pulled = Pulled::default();
        for (at, need) in needs.iter().enumerate() {
            if need.hold.is_empty() && need.compute.is_empty() {
                continue;
            }
            if self.values[at].alias().is_some() {
                continue;
            }
            let (done, rest) = self.values.split_at_mut(at);
            let value = &mut rest[0];
            let place = &mut self.places[at];
            store::noted(value, place, !need.compute.is_empty(), recording);
            let marks = eval::Marks {
                at: place
                    .segments
                    .iter()
                    .skip(1)
                    .map(|(start, _)| *start)
                    .collect(),
                every: recording
                    .stores(place.fork, place.target)
                    .then(|| recording.mark_every()),
            };
            let (priced, waves) = eval::compute(value, need, (done, &marks), &self.profile)?;
            pulled.priced += priced;
            pulled.waves += waves;
            let computed: Vec<Extent> = need.compute.iter().collect();
            store::stored(value, place, &computed, recording);
            for (read, count) in store::reached(value, place, &need.compute) {
                store::reread(&done[read], &mut self.places[read], count, recording);
            }
        }
        pulled.most_bytes = self.bytes();
        Ok(pulled)
    }

    /// Drops what no later window reads: `future` is the rest of the root's range, `keep` more
    /// the root holds besides, and `since` where the output is read from.
    pub(crate) fn release(&mut self, future: Option<Extent>, keep: Extent, since: i64) {
        let needs = match future {
            Some(window) => self.demand(window),
            None => vec![Need::default(); self.values.len()],
        };
        self.released(needs, keep, since);
    }

    /// A wanted value keeps everything, as a wanted reader's rerun reads it from its start;
    /// the target's own value, which no wanted value reads, keeps only the output's samples.
    fn released(&mut self, needs: Vec<Need>, keep: Extent, since: i64) {
        let (mut root, mut by) = (self.root, 0);
        while let Some((read, shift)) = self.values[root].alias() {
            (root, by) = (read, by + shift);
        }
        let output = !self
            .wanted
            .iter()
            .any(|w| *w != root && self.values[*w].reads.contains(&root));
        for (at, need) in needs.into_iter().enumerate() {
            let whole = self.whole(at) && !(output && at == root);
            if self.values[at].alias().is_some() || whole {
                continue;
            }
            let mut kept = need.hold;
            if at == root {
                kept.add(keep.shifted(by));
                if self.whole(at) {
                    kept.add(Extent::new(since, i64::MAX).shifted(by));
                }
            }
            let value = &mut self.values[at];
            if let (Kind::Program(program), Some(end)) = (&value.kind, value.end()) {
                kept.add(Extent::new(end.saturating_sub(program.own), end));
            }
            value.retain(&kept);
        }
    }

    /// A wanted value, or one a wanted value only moves: held over the whole range.
    fn whole(&self, at: usize) -> bool {
        self.wanted.iter().any(|w| {
            let mut v = *w;
            loop {
                if v == at {
                    return true;
                }
                match self.values[v].alias() {
                    Some((read, _)) => v = read,
                    None => return false,
                }
            }
        })
    }

    pub(crate) fn bytes(&self) -> usize {
        self.values.iter().map(Value::bytes).sum()
    }

    /// A value's samples over `over`, zero wherever none is held.
    pub(crate) fn samples(&self, at: usize, over: Extent) -> Buffer {
        eval::samples_of(&self.values, at, over)
    }

    /// `at`'s program with another renderer, over `over`.
    pub(crate) fn rerun(
        &self,
        at: usize,
        renderer: &NodeRenderer,
        over: Extent,
    ) -> Result<Buffer, EngineError> {
        eval::rerun(&self.values, at, renderer, over)
    }

    /// A program that only reads another value at a whole sample holds that value's label.
    pub(crate) fn label(&self, at: usize) -> Label {
        let value = &self.values[at];
        if let Kind::Program(program) = &value.kind
            && let NodeRenderer::Read {
                slot: Slot::Read(slot),
                ..
            } = program.renderer
        {
            return self.label(value.reads[slot.0 as usize]);
        }
        value
            .label
            .clone()
            .unwrap_or_else(|| Label::measured(self.profile.name, value.grid.rate))
    }

    /// What `need` costs each value, as computing it pays.
    pub(crate) fn price(&self, needs: &[Need]) -> Vec<u128> {
        self.values
            .iter()
            .zip(needs)
            .map(|(value, need)| eval::price(value, &need.compute))
            .collect()
    }
}

/// What one pull computed: its price, and each value it asked, whether it computed any of it.
#[derive(Default)]
pub(crate) struct Pulled {
    pub(crate) priced: u128,
    pub(crate) waves: u128,
    /// The most bytes the table held once a block was computed.
    pub(crate) most_bytes: usize,
}

struct Building<'a> {
    tys: &'a Typing,
    supports: &'a Supports<'a>,
    profile: &'a Profile,
    fine: i128,
    /// Every read its own value rather than one per identity.
    apart: bool,
    /// Values made apart so far, each its own key.
    copies: u64,
    /// The nodes whose reads are being built, apart.
    reading: Vec<NodeId>,
    named: BTreeMap<NodeId, Hash>,
    keys: HashMap<Key, usize>,
    open: Vec<Key>,
    values: Vec<Value>,
    nodes: BTreeMap<NodeId, usize>,
    moved: f64,
}

impl Building<'_> {
    fn node(&mut self, id: NodeId) -> Result<usize, EngineError> {
        if let Some(at) = self.nodes.get(&id) {
            return Ok(*at);
        }
        let grid = self.grid(id);
        let key = Key {
            identity: refs::identity_in(self.tys, id, &mut self.named)?,
            step: step(grid),
        };
        let at = self.value(key, Source::Node(id), grid, self.tys.name(id))?;
        self.nodes.insert(id, at);
        Ok(at)
    }

    fn grid(&self, id: NodeId) -> Grid {
        let grid = self.tys.grid(id);
        let step = grid
            .step()
            .div(crate::time::Q::new(self.fine, 1).expect("a factor"))
            .expect("a step");
        Grid::stepping(grid.rate, step)
    }

    /// Its support on its own grid, where it is `fine` times finer, every sample it may cover.
    fn support(&self, id: NodeId) -> Extent {
        let held = self.supports.of(id);
        let fine = i64::try_from(self.fine).expect("a small factor");
        match (held.is_empty(), fine) {
            (true, _) | (_, 1) => held,
            _ => {
                let edge = |n: i64| match n {
                    i64::MIN | i64::MAX => n,
                    n => n.saturating_mul(fine),
                };
                Extent::new(edge(held.start).saturating_sub(fine), edge(held.end))
            }
        }
    }

    fn source(&mut self, source: &Source, grid: Grid, name: &str) -> Result<usize, EngineError> {
        let identity = match (source, self.apart) {
            (Source::Node(id), false) => return self.node(*id),
            (Source::Node(id), true) => {
                if self.reading.contains(id) {
                    return Err(refs::cyclic(self.tys, *id));
                }
                refs::identity_in(self.tys, *id, &mut self.named)?
            }
            (Source::Formula(form), _) => refs::formula_identity(form),
        };
        let grid = match source {
            Source::Node(id) => self.grid(*id),
            Source::Formula(_) => grid,
        };
        let key = Key {
            identity: match self.apart {
                false => identity,
                true => {
                    self.copies += 1;
                    crate::cache::mixed(identity, &[self.copies])
                }
            },
            step: step(grid),
        };
        let name = match source {
            Source::Node(id) => self.tys.name(*id),
            Source::Formula(_) => name,
        };
        if let Source::Node(id) = source {
            self.reading.push(*id);
        }
        let at = self.value(key, source.clone(), grid, name);
        if let Source::Node(_) = source {
            self.reading.pop();
        }
        at
    }

    fn value(
        &mut self,
        key: Key,
        source: Source,
        grid: Grid,
        name: &str,
    ) -> Result<usize, EngineError> {
        if let Some(at) = self.keys.get(&key) {
            return Ok(*at);
        }
        if self.open.contains(&key) {
            let Source::Node(id) = source else {
                unreachable!("a formula reads no node");
            };
            return Err(refs::cyclic(self.tys, id));
        }
        self.open.push(key);
        let built = self.built(key, &source, grid, name);
        self.open.pop();
        let value = built?;
        self.values.push(value);
        let at = self.values.len() - 1;
        self.keys.insert(key, at);
        Ok(at)
    }

    fn built(
        &mut self,
        key: Key,
        source: &Source,
        grid: Grid,
        name: &str,
    ) -> Result<Value, EngineError> {
        let tys = self.tys;
        let (node, support, width) = match source {
            Source::Node(id) => (
                Some(*id),
                self.support(*id),
                usize::from(tys.ty(*id).width).max(1),
            ),
            Source::Formula(form) => (None, self.supports.formula(&form.body, grid), 1),
        };
        let mut value = Value {
            key,
            node,
            name: name.to_string(),
            grid,
            width,
            support,
            period: None,
            kind: Kind::Istft,
            reads: Vec::new(),
            held: Held::Segments(Vec::new()),
            evaluated: Vec::new(),
            label: None,
            switches: Vec::new(),
            pure: true,
        };
        let Source::Node(id) = source else {
            let Source::Formula(form) = source else {
                unreachable!("a node or a formula");
            };
            let sum = sva_formula::normalize_closed_form(form).ok();
            return self.formula(value, sum, Some(form));
        };
        let id = *id;
        match (tys.ty(id).held, tys.value(id)) {
            (Representation::Frames, Typed::Cast(Cast::Stft { window, hop }, of)) => {
                value.kind = Kind::Frames {
                    window: *window,
                    hop: *hop,
                };
                value.reads = vec![self.node(*of)?];
                value.held = Held::Frames(None);
                value.support = self.support(*of);
                match value.support.is_bounded() {
                    true => Ok(value),
                    false => Err(unbounded(&value.name)),
                }
            }
            (_, Typed::Cast(Cast::Istft, frames)) => {
                value.reads = vec![self.node(*frames)?];
                Ok(value)
            }
            (_, Typed::ClosedForm(form)) if form.var == Var::F => self.spectrum(value, id),
            (_, Typed::Op { name, .. }) if tys.var(id) == Var::F => {
                Err(refs::across(tys, id, name))
            }
            (_, Typed::Cast(Cast::Fourier | Cast::IFourier, _)) if tys.var(id) == Var::F => {
                self.spectrum(value, id)
            }
            (_, Typed::ClosedForm(form)) if refs::nodes_in(&form.body).is_empty() => {
                let sum = sva_formula::normalize_closed_form(form).ok();
                self.formula(value, sum, Some(form))
            }
            (_, Typed::Cast(Cast::Fourier | Cast::IFourier, _)) => {
                let sum = refs::spectral_sum_of(tys, id, Var::T)?;
                self.formula(value, Some(sum), None)
            }
            _ => self.program(value, id),
        }
    }

    /// A form in `f` is, on the grid, the form in `t` its dual is; one with no dual is its
    /// inverse spectrum over what it is asked for.
    fn spectrum(&mut self, mut value: Value, id: NodeId) -> Result<Value, EngineError> {
        if let Ok(sum) = refs::spectral_sum_of(self.tys, id, Var::T) {
            return self.formula(value, Some(sum), None);
        }
        value.kind = Kind::Spectrum(Box::new(refs::spectral_sum_of(self.tys, id, Var::F)?));
        Ok(value)
    }

    /// Its rows where its step makes a whole rate, else its formula at each instant.
    fn formula(
        &mut self,
        mut value: Value,
        sum: Option<sva_formula::SpectralSum>,
        written: Option<&ClosedForm>,
    ) -> Result<Value, EngineError> {
        let grid = value.grid;
        let rows = whole_rate(grid).map(|rate| match (&sum, written) {
            (Some(sum), written) => {
                Rows::of_spectral_sum_or_point(sum, written, rate, self.profile)
            }
            (None, Some(form)) => Rows::of(form, rate, self.profile),
            (None, None) => unreachable!("a formula is a sum or a written form"),
        });
        match rows {
            Some(Ok(rows)) => {
                value.width = rows.width();
                value.period = period::period(written, &rows, grid);
                value.label = Some(rows.label(self.profile));
                value.kind = Kind::Rows(Box::new(rows));
                Ok(value)
            }
            Some(Err(e)) => Err(eval::collapse_refused(&value.name, &e)),
            None => {
                let band = sva_samples::Audible::on(self.profile, grid);
                let refused =
                    |e: &sva_samples::CollapseError| eval::collapse_refused(&value.name, e);
                let summed = sum.as_ref().map(|sum| truncate_spectral_sum(sum, band));
                let formula = match (summed, written) {
                    (Some(Ok(sum)), _) => Formula::Sum(Box::new(sum)),
                    (_, Some(form)) => Formula::Written(Box::new(
                        truncate_written(&form.body, band).map_err(|e| refused(&e))?,
                    )),
                    (Some(Err(e)), None) => return Err(refused(&e)),
                    (None, None) => unreachable!("a formula is a sum or a written form"),
                };
                let renderer = NodeRenderer::Formula {
                    formula,
                    width: value.width,
                    time: Box::new(NodeRenderer::Time),
                };
                self.running(value, renderer, Vec::new(), Vec::new(), None)
            }
        }
    }

    /// A closed form's program point-samples it; any other program is a reading of samples.
    fn program(&mut self, mut value: Value, id: NodeId) -> Result<Value, EngineError> {
        if self.tys.ty(id).is_closed_form() {
            value.label = Some(Label::new(
                sva_samples::Source::Measured,
                self.profile.name,
                value.grid.rate,
                sva_samples::Detail::Point {
                    rule: sva_samples::Rule::PointSampled,
                    alias_db: None,
                },
            ));
        }
        let built = program::of(self.tys, self.supports, (id, value.grid), self.profile)?;
        self.moved = self.moved.max(built.moved);
        let grid = value.grid;
        let mut reads = Vec::with_capacity(built.reads.len());
        for source in &built.reads {
            reads.push(self.source(source, grid, &value.name)?);
        }
        let endless = |slot: Slot| match slot {
            Slot::Own => true,
            Slot::Read(at) => !self.values[reads[at.0 as usize]].support.is_bounded(),
        };
        let renderer = built.renderer.stepwise(&endless);
        let stateful = !built.sites.is_empty() || reads_own(&renderer);
        let start = stateful.then(|| {
            self.supports
                .state_start(id, id)
                .unwrap_or(value.support.start)
        });
        value.switches = match stateful && self.fine == 1 {
            true => refs::switches(self.tys, id, &mut self.named)?,
            false => Vec::new(),
        };
        self.running(value, renderer, reads, built.sites, start)
    }

    fn running(
        &mut self,
        mut value: Value,
        renderer: NodeRenderer,
        reads: Vec<usize>,
        sites: Vec<sva_samples::Site>,
        start: Option<i64>,
    ) -> Result<Value, EngineError> {
        let widths = reads.iter().map(|at| self.values[*at].width).collect();
        let live: Vec<Extent> = reads.iter().map(|at| self.values[*at].support).collect();
        let layout = sva_samples::machine::ops::Layout {
            grid: value.grid,
            width: value.width,
            read_widths: widths,
            sites,
        };
        let from = start.map_or(value.support.start, |s| s.min(value.support.start));
        let spanned = Spanned::new(&renderer, &layout, (from, value.support.end), &live)
            .map_err(|e| eval::sample_refused(&value.name, &e))?;
        if start.is_some() {
            value.held = Held::Run(Tape::new(value.width, 0, from));
        }
        let alias = match (&renderer, start) {
            (
                NodeRenderer::Read {
                    slot: Slot::Read(slot),
                    map,
                },
                None,
            ) if map.a == 1 && map.d == 1 => Some((slot.0 as usize, map.at(0))),
            _ => None,
        };
        value.kind = Kind::Program(Box::new(Program {
            alias,
            own: own_reach(&renderer),
            renderer,
            spanned,
            layout,
            start,
            machine: None,
            marks: Default::default(),
        }));
        value.reads = reads;
        Ok(value)
    }
}

/// The value `at` only moves, through every alias between.
fn aliased(values: &[Value], mut at: usize) -> usize {
    while let Some((read, _)) = values[at].alias() {
        at = read;
    }
    at
}

/// Each value's key in the store; a value two others read is a fork, the root the target.
fn places(values: &[Value], root: usize, profile: &Profile) -> Vec<store::Place> {
    let mut readers = vec![0usize; values.len()];
    for value in values {
        let mut read = value.reads.clone();
        read.sort_unstable();
        read.dedup();
        for at in read {
            readers[at] += 1;
        }
    }
    values
        .iter()
        .enumerate()
        .map(|(at, value)| {
            let key = |identity| {
                crate::cache::value_key(
                    identity,
                    value.key.step,
                    value.grid.rate,
                    value.width,
                    profile,
                )
            };
            store::Place {
                key: key(value.key.identity),
                segments: segments(&value.switches, key(value.key.identity), key),
                fork: readers[at] >= 2,
                target: at == root,
                slot: None,
                unread: leaf_reads(values, value),
                reached: 0,
                looked: false,
                prefixed: false,
                noted: None,
            }
        })
        .collect()
}

/// A run's segments: each switch starts one, keyed by the identity before the next, and a
/// switch that changes no key starts none.
fn segments(switches: &[(i64, Hash)], whole: Hash, key: impl Fn(Hash) -> Hash) -> Vec<(i64, Hash)> {
    let (mut starts, mut keys) = (vec![i64::MIN], Vec::new());
    for (at, before) in switches {
        let held = key(*before);
        if keys.last() != Some(&held) {
            keys.push(held);
            starts.push(*at);
        }
    }
    match keys.last() == Some(&whole) {
        true => {
            starts.pop();
        }
        false => keys.push(whole),
    }
    starts.into_iter().zip(keys).collect()
}

/// Each distinct read a value's program makes, however many leaves share a slot; a value of no
/// program reads each of its reads once, whole. A read through an alias reads the value it
/// moves, and an alias reads nothing of its own.
fn leaf_reads(values: &[Value], value: &Value) -> Vec<store::Unread> {
    let Kind::Program(program) = &value.kind else {
        return value
            .reads
            .iter()
            .map(|at| store::Unread {
                leaf: None,
                read: aliased(values, *at),
                count: 1,
            })
            .collect();
    };
    if program.alias.is_some() {
        return Vec::new();
    }
    let mut out: Vec<store::Unread> = Vec::new();
    program::leaves(&program.renderer, &mut |leaf| {
        let (NodeRenderer::Read {
            slot: Slot::Read(at),
            ..
        }
        | NodeRenderer::Indexed {
            slot: Slot::Read(at),
            ..
        }) = leaf
        else {
            return;
        };
        match out.iter_mut().find(|u| u.leaf.as_ref() == Some(leaf)) {
            Some(held) => held.count += 1,
            None => out.push(store::Unread {
                leaf: Some(leaf.clone()),
                read: aliased(values, value.reads[at.0 as usize]),
                count: 1,
            }),
        }
    });
    out
}

/// A short-time transform reads its input whole.
fn unbounded(name: &str) -> EngineError {
    EngineError::refused(crate::error::Diagnostic {
        code: "engine.unbounded_extent".to_string(),
        message: format!("`{name}` reads its input over every instant, and that input never ends"),
        location: crate::error::Located::at(name, None),
        help: "crop what a short-time transform takes to a window".to_string(),
    })
}

fn step(grid: Grid) -> (i128, i128) {
    (grid.a, grid.d)
}

/// The rate a grid's step makes, where it is a whole one.
fn whole_rate(grid: Grid) -> Option<u32> {
    let rate = crate::time::Q::int(i64::from(grid.rate)).div(grid.step())?;
    rate.is_integer()
        .then(|| u32::try_from(rate.num()).ok())
        .flatten()
}

fn reads_own(renderer: &NodeRenderer) -> bool {
    own_reach(renderer) > 0 || {
        let mut found = false;
        program::leaves(renderer, &mut |leaf| {
            found |= matches!(
                leaf,
                NodeRenderer::Read {
                    slot: Slot::Own,
                    ..
                } | NodeRenderer::Indexed {
                    slot: Slot::Own,
                    ..
                }
            );
        });
        found
    }
}

/// How far back its own past is read: past any sample where an index has no bound.
fn own_reach(renderer: &NodeRenderer) -> i64 {
    let mut back = 0i64;
    program::leaves(renderer, &mut |leaf| match leaf {
        NodeRenderer::Read {
            slot: Slot::Own,
            map,
        } => back = back.max(map.least().saturating_neg().max(0)),
        NodeRenderer::Indexed {
            slot: Slot::Own,
            reach,
            ..
        } => {
            let least = reach.map_or(i64::MIN, |(least, _)| least);
            back = back.max(least.saturating_neg().max(0));
        }
        _ => {}
    });
    back
}
