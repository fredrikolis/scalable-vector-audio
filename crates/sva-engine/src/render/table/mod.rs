// Concern: the one table of values, keyed by identity and step, that renders, streams, prices and logs read | Non-concern: typing, readings off samples | IO: (Typing, roots) -> Table, samples

mod demand;
pub(crate) mod edit;
mod eval;
#[cfg(test)]
mod laws;
mod period;
pub(crate) mod program;
pub(crate) mod segments;
mod store;
pub(crate) mod support;
mod value;
mod values;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use sva_formula::{ClosedForm, Hash, Held as Representation, NodeId, Var};
use sva_samples::{
    Buffer, Extent, Formula, Grid, Label, NodeRenderer, Profile, Rows, Slot, Spanned, Tape,
    Written, truncate_spectral_sum_read, truncate_written,
};

pub(crate) use demand::Need;
pub(crate) use value::{Held, Key, Kind, Value};
pub(crate) use values::Values;

use crate::cache::{Recording, Stored};
use crate::cast::Cast;
use crate::error::EngineError;
use crate::refs;
use crate::time::Lattice;
use crate::typing::{Typing, Value as Typed};
use program::Source;
use support::{Memo, Supports};
use value::Program;

/// Every value the root and each wanted node read, once per identity and step, each in its
/// slot while a reader, a node or the root holds it.
pub(crate) struct Table {
    pub(crate) values: Values,
    nodes: BTreeMap<NodeId, usize>,
    pub(crate) root: usize,
    /// Each node a reading holds over the range, in its own time.
    pub(crate) wanted: Vec<usize>,
    /// What each value costs over the whole range, as a pull pays it.
    pub(crate) planned: Vec<u128>,
    profile: Profile,
    /// Each node pruned under the profile's level, and the sample it is zero from.
    cuts: BTreeMap<NodeId, i64>,
    pub(crate) built: usize,
    pub(crate) supports: Memo,
    rooted: bool,
    /// Each value started silent since the table last settled.
    silenced: Vec<usize>,
    /// What a build made and named, until settled or let go.
    draft: Draft,
}

#[derive(Default)]
struct Draft {
    made: Vec<usize>,
    noded: Vec<(NodeId, Option<usize>)>,
}

/// How much finer than its own step each value is, the nodes standing as values of their own,
/// and the samples the store holds of each.
type Bounds<'b> = (
    i128,
    &'b BTreeSet<NodeId>,
    &'b BTreeMap<NodeId, Arc<Stored>>,
);

impl Table {
    pub(crate) fn new(profile: &Profile) -> Table {
        Table {
            values: Values::default(),
            nodes: BTreeMap::new(),
            root: 0,
            wanted: Vec::new(),
            planned: Vec::new(),
            profile: *profile,
            cuts: BTreeMap::new(),
            built: 0,
            supports: Memo::default(),
            rooted: false,
            silenced: Vec::new(),
            draft: Draft::default(),
        }
    }

    /// Every value `root` and `wanted` read, each once per identity and step, readers after
    /// the values they read.
    pub(crate) fn build(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
    ) -> Result<Table, EngineError> {
        let none = (&BTreeSet::new(), Memo::default());
        Table::bounded(tys, (root, wanted), profile, none)
    }

    /// The same, each of `bounds` a value of its own wherever it is read, never inlined, over
    /// the supports `found` for its range.
    pub(crate) fn bounded(
        tys: &Typing,
        (root, wanted): (NodeId, &[NodeId]),
        profile: &Profile,
        (bounds, found): (&BTreeSet<NodeId>, Memo),
    ) -> Result<Table, EngineError> {
        let mut table = Table::new(profile);
        table.supports = found;
        table.built(tys, (root, wanted), (1, bounds, &BTreeMap::new()), false)
    }

    /// Every read its own value, as if each were written out where it is read.
    #[cfg(test)]
    pub(crate) fn apart(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
    ) -> Result<Table, EngineError> {
        let none = (1, &BTreeSet::new(), &BTreeMap::new());
        Table::new(profile).built(tys, (root, wanted), none, true)
    }

    /// Every value `fine` times finer than its own step: a closed form's reference.
    pub(crate) fn finer(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
        fine: i128,
    ) -> Result<Table, EngineError> {
        let finer = (fine, &BTreeSet::new(), &BTreeMap::new());
        Table::new(profile).built(tys, (root, wanted), finer, false)
    }

    fn built(
        mut self,
        tys: &Typing,
        (root, wanted): (NodeId, &[NodeId]),
        bounds: Bounds<'_>,
        apart: bool,
    ) -> Result<Table, EngineError> {
        let mut held = Vec::with_capacity(wanted.len() + 1);
        for id in std::iter::once(&root).chain(wanted) {
            held.push(self.grown(tys, *id, (bounds, apart))?);
        }
        self.draft = Draft::default();
        self.held_as(held[0], held[1..].to_vec());
        Ok(self)
    }

    /// The value for `id` and each it reads the table lacks, built; a node `prefixes` names
    /// stands on the store's samples, its live value continuing past.
    pub(crate) fn grow(
        &mut self,
        tys: &Typing,
        id: NodeId,
        prefixes: &BTreeMap<NodeId, Arc<Stored>>,
    ) -> Result<usize, EngineError> {
        self.built = 0;
        let bounds = (1, &BTreeSet::new(), prefixes);
        self.grown(tys, id, (bounds, false))
    }

    fn grown(
        &mut self,
        tys: &Typing,
        id: NodeId,
        ((fine, bounds, prefixes), apart): (Bounds<'_>, bool),
    ) -> Result<usize, EngineError> {
        let memo = std::mem::take(&mut self.supports);
        let profile = self.profile;
        let supports = Supports::over(tys, &profile, Some(&memo));
        let mut building = Building {
            tys,
            supports: &supports,
            profile: &profile,
            fine,
            bounds,
            prefixes,
            apart,
            copies: 0,
            reading: Vec::new(),
            open: Vec::new(),
            table: self,
        };
        let built = building.node(id);
        let cuts = supports.cuts();
        let more = supports.into_memo();
        self.supports = memo;
        self.supports.extend(more);
        self.cuts.extend(cuts);
        built
    }

    /// `root` and `wanted` held in place of what was.
    fn held_as(&mut self, root: usize, wanted: Vec<usize>) {
        let (old, before) = (self.root, std::mem::take(&mut self.wanted));
        let had = std::mem::replace(&mut self.rooted, true);
        for at in std::iter::once(root).chain(wanted.iter().copied()) {
            self.values.hold(at);
        }
        if had {
            let target = aliased(&self.values, old);
            self.values.place_mut(target).target = false;
        }
        let target = aliased(&self.values, root);
        self.values.place_mut(target).target = true;
        (self.root, self.wanted) = (root, wanted);
        if had {
            let dropped: Vec<usize> = std::iter::once(old).chain(before).collect();
            let gone = self.unheld(dropped);
            self.freed(gone);
        }
    }

    /// Each value a volatile parameter reaches keeps one entry, its last, under its slot.
    pub(crate) fn slots(&mut self, slot: impl Fn(NodeId) -> Option<Hash>) {
        for at in self.values.ordered().collect::<Vec<_>>() {
            let held = self.values[at].node.and_then(&slot);
            self.values.place_mut(at).slot = held;
        }
    }

    /// Prices every value over `range` before a sample is computed; a stored one costs what
    /// computing it did.
    pub(crate) fn plan(&mut self, range: Extent) {
        let needs = self.demand(range);
        self.planned = self.price(&needs);
        for (at, value) in self.values.iter() {
            if let Kind::Resident(stored) = &value.kind {
                self.planned[at] += stored.priced;
            }
        }
    }

    /// The most seconds any read was moved to a whole sample.
    pub(crate) fn moved(&self) -> f64 {
        let moved = self.values.iter().map(|(_, value)| value.moved);
        moved.fold(0.0, f64::max)
    }

    pub(crate) fn pruned(&self, tys: &Typing) -> sva_samples::Pruned {
        sva_samples::Pruned {
            db: self.profile.prune_db,
            cuts: self.cuts_of(tys, |_| true),
        }
    }

    /// Each cut of a node `within` names, those a stored node carries among them, by name.
    pub(crate) fn cuts_of(
        &self,
        tys: &Typing,
        within: impl Fn(&str) -> bool,
    ) -> Vec<(String, i64)> {
        let own = self
            .cuts
            .iter()
            .map(|(id, at)| (tys.name(*id).to_string(), *at));
        let carried = self
            .values
            .iter()
            .filter_map(|(_, value)| match &value.kind {
                Kind::Resident(stored) if within(&value.name) => Some(stored.cuts.iter().cloned()),
                _ => None,
            });
        let cuts = own
            .filter(|(name, _)| within(name))
            .chain(carried.flatten());
        cuts.collect::<BTreeSet<_>>().into_iter().collect()
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
        let needs = self.demand(window);
        let spans: Vec<(usize, Extent)> = self
            .values
            .iter()
            .filter(|(_, value)| matches!(&value.kind, Kind::Program(p) if p.stateful()))
            .filter_map(|(at, _)| {
                let need = &needs[at];
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
                Some(window.start),
            );
            from = to;
        }
        Ok(pulled)
    }

    /// Silences each stateful value `window` asks from a sample its run has not reached, from
    /// that sample, readers first: nothing before it is computed. Those it silenced.
    pub(crate) fn skipped(&mut self, window: Extent) -> Result<Vec<usize>, EngineError> {
        let mut silenced = Vec::new();
        loop {
            let needs = self.demand(window);
            let behind = self.values.ordered().rev().find(|at| {
                let need = &needs[*at];
                let stateful = matches!(&self.values[*at].kind, Kind::Program(p) if p.stateful());
                let asked = need.hold.hull().start;
                stateful && !need.compute.is_empty() && need.compute.hull().start < asked
            });
            let Some(at) = behind else {
                return Ok(silenced);
            };
            let from = needs[at].hold.hull().start;
            let value = &mut self.values[at];
            value
                .silent_from(from)
                .map_err(|e| eval::sample_refused(&value.name, &e))?;
            silenced.push(at);
            self.silenced.push(at);
        }
    }

    fn pulled(
        &mut self,
        asked: &[(usize, Extent)],
        recording: &mut Recording,
    ) -> Result<Pulled, EngineError> {
        let mut needs = demand::demand(&self.values, asked);
        let order: Vec<usize> = self.values.ordered().collect();
        loop {
            let mut loaded = false;
            for at in order.iter().copied() {
                let (value, place) = self.values.placed(at);
                if needs[at].hold.is_empty() || place.looked || !value.pure {
                    continue;
                }
                loaded |= store::load(value, place, recording);
            }
            if !loaded {
                break;
            }
            needs = demand::demand(&self.values, asked);
        }
        let mut pulled = Pulled::default();
        for at in order {
            let need = &needs[at];
            if need.hold.is_empty() && need.compute.is_empty() {
                continue;
            }
            if self.values[at].alias().is_some() {
                continue;
            }
            let mut lifted = self.values.lift(at);
            let computed = self.computed(at, &mut lifted, need, recording);
            self.values.put(at, lifted);
            let (priced, waves) = computed?;
            pulled.priced += priced;
            pulled.waves += waves;
        }
        pulled.most_bytes = self.bytes();
        Ok(pulled)
    }

    fn computed(
        &mut self,
        at: usize,
        value: &mut Value,
        need: &Need,
        recording: &mut Recording,
    ) -> Result<(u128, u128), EngineError> {
        let place = self.values.place_mut(at);
        store::noted(value, place, !need.compute.is_empty(), recording);
        let marks = eval::Marks {
            at: place
                .segments
                .iter()
                .skip(1)
                .map(|(start, _)| *start)
                .collect(),
            every: recording
                .keeps(place.fork, place.target, place.offered)
                .then(|| recording.mark_every()),
        };
        let done = eval::compute(value, need, (&self.values, &marks), &self.profile)?;
        let computed: Vec<Extent> = need.compute.iter().collect();
        let place = self.values.place_mut(at);
        store::stored(value, place, &computed, recording);
        for (read, count) in store::reached(value, place, &need.compute) {
            let (read, place) = self.values.placed(read);
            store::reread(read, place, count, recording);
        }
        Ok(done)
    }

    /// Drops what no later window reads: `future` is the rest of the root's range, `keep` more
    /// the root holds besides, and `since` where the output is read from, if it is.
    pub(crate) fn release(&mut self, future: Option<Extent>, keep: Extent, since: Option<i64>) {
        let needs = match future {
            Some(window) => self.demand(window),
            None => vec![Need::default(); self.values.span()],
        };
        self.released(needs, keep, since);
    }

    /// A wanted value keeps everything, as a wanted reader's rerun reads it from its start;
    /// the target's own value, which no wanted value reads, keeps only the output's samples.
    fn released(&mut self, mut needs: Vec<Need>, keep: Extent, since: Option<i64>) {
        let (mut root, mut by) = (self.root, 0);
        while let Some((read, shift)) = self.values[root].alias() {
            (root, by) = (read, by + shift);
        }
        let output = !self
            .wanted
            .iter()
            .any(|w| *w != root && self.values[*w].reads.contains(&root));
        for at in self.values.ordered().collect::<Vec<_>>() {
            let need = std::mem::take(&mut needs[at]);
            let whole = self.whole(at) && !(output && at == root);
            let value = &self.values[at];
            let stored = matches!(value.kind, Kind::Resident { .. }) && value.reads.is_empty();
            if value.alias().is_some() || whole || stored {
                continue;
            }
            let mut kept = need.hold;
            if at == root {
                kept.add(keep.shifted(by));
                if let Some(since) = since.filter(|_| self.whole(at)) {
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
        self.values.iter().map(|(_, value)| value.bytes()).sum()
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
            } = *program.renderer
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
        let mut out = vec![0; self.values.span()];
        for (at, value) in self.values.iter() {
            out[at] = eval::price(value, &needs[at].compute);
        }
        out
    }

    /// A value made, holding what it reads.
    fn make(&mut self, value: Value) -> usize {
        let place = place(&self.values, &value, &self.profile);
        let at = self.values.push(value, place);
        if self.planned.len() < self.values.span() {
            self.planned.resize(self.values.span(), 0);
        }
        self.planned[at] = 0;
        self.draft.made.push(at);
        at
    }

    /// `id` standing for the value at `at`, holding it.
    fn name(&mut self, id: NodeId, at: usize) {
        self.values.hold(at);
        let old = self.nodes.insert(id, at);
        if let Some(old) = old {
            self.values.release(old);
        }
        self.draft.noded.push((id, old));
    }

    /// Each of `dropped` held once less; each value nothing holds now, readers first.
    fn unheld(&mut self, dropped: Vec<usize>) -> Vec<usize> {
        let mut open = dropped;
        let mut gone = Vec::new();
        while let Some(at) = open.pop() {
            if self.values.release(at) {
                gone.push(at);
                open.extend(self.values[at].reads.iter().copied());
            }
        }
        gone
    }

    fn freed(&mut self, gone: Vec<usize>) {
        for at in gone {
            self.values.remove(at);
            self.planned[at] = 0;
        }
    }

    /// `root` held, the latest build kept, each node `freed` unnamed: what nothing holds goes,
    /// a value made first taking the state of what it continues.
    pub(crate) fn settled(
        &mut self,
        root: usize,
        freed: &[NodeId],
        (now, live): (i64, bool),
    ) -> edit::Carried {
        let draft = std::mem::take(&mut self.draft);
        for at in std::mem::take(&mut self.silenced) {
            if self.values.holds(at) {
                self.values[at].silent = None;
            }
        }
        let newest: HashMap<usize, NodeId> = draft
            .noded
            .iter()
            .map(|(id, _)| (self.nodes[id], *id))
            .collect();
        let mut dropped = Vec::with_capacity(freed.len());
        for id in freed {
            let Some(at) = self.nodes.remove(id) else {
                continue;
            };
            if self.values[at].node == Some(*id) {
                self.values[at].node = newest.get(&at).copied();
            }
            dropped.push(at);
        }
        let old = self.rooted.then_some(self.root);
        dropped.extend(old);
        self.values.hold(root);
        if let Some(old) = old {
            let target = aliased(&self.values, old);
            self.values.place_mut(target).target = false;
        }
        let target = aliased(&self.values, root);
        self.values.place_mut(target).target = true;
        (self.root, self.rooted) = (root, true);
        let going = self.unheld(dropped);
        let made: Vec<usize> = draft
            .made
            .into_iter()
            .filter(|at| self.values.held(*at))
            .collect();
        let carried = edit::carried(&mut self.values, (&made, &going), (old, root), (now, live));
        self.silenced.extend(carried.silent.iter().copied());
        self.freed(going);
        self.forget(freed);
        carried
    }

    /// The latest build let go, and what was found of `freed` forgotten.
    pub(crate) fn abort(&mut self, freed: &[NodeId]) {
        let draft = std::mem::take(&mut self.draft);
        for (id, old) in draft.noded.into_iter().rev() {
            let at = self.nodes.remove(&id).expect("a node this build named");
            self.values.release(at);
            if let Some(old) = old {
                self.nodes.insert(id, old);
                self.values.hold(old);
            }
        }
        for at in draft.made.iter().rev() {
            for read in self.values[*at].reads.clone() {
                self.values.release(read);
            }
        }
        self.freed(draft.made.into_iter().rev().collect());
        self.forget(freed);
    }

    pub(crate) fn made(&self) -> &[usize] {
        &self.draft.made
    }

    fn forget(&mut self, freed: &[NodeId]) {
        for id in freed {
            self.cuts.remove(id);
        }
        self.supports.forget(freed);
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
    bounds: &'a BTreeSet<NodeId>,
    prefixes: &'a BTreeMap<NodeId, Arc<Stored>>,
    /// Every read its own value rather than one per identity.
    apart: bool,
    /// Values made apart so far, each its own key.
    copies: u64,
    /// The nodes whose reads are being built, apart.
    reading: Vec<NodeId>,
    open: Vec<Key>,
    table: &'a mut Table,
}

impl Building<'_> {
    fn node(&mut self, id: NodeId) -> Result<usize, EngineError> {
        if let Some(at) = self.table.nodes.get(&id) {
            return Ok(*at);
        }
        let grid = self.grid(id);
        let key = Key {
            identity: refs::identity(self.tys, id)?,
            step: step(grid),
        };
        let mut at = self.value(key, Source::Node(id), grid, self.tys.name(id))?;
        if let Some(stored) = self.prefixes.get(&id) {
            at = self.prefix(at, stored);
        }
        self.table.name(id, at);
        Ok(at)
    }

    /// The stored samples of `live`'s node, reading `live` for all they miss; none where they
    /// were written on another grid or width.
    fn prefix(&mut self, live: usize, stored: &Arc<Stored>) -> usize {
        let of = &self.table.values[live];
        if stored.grid != of.grid || usize::from(stored.width) != of.width {
            return live;
        }
        let key = Key {
            identity: crate::cache::mixed(of.key.identity, &[PREFIX]),
            step: of.key.step,
        };
        if let Some(at) = self.table.values.of(&key) {
            return at;
        }
        let mut value = Value {
            key,
            node: of.node,
            name: of.name.clone(),
            grid: of.grid,
            width: of.width,
            whole: of.support(),
            silent: None,
            period: None,
            kind: Kind::Resident(Arc::clone(stored)),
            reads: vec![live],
            held: Held::Segments(Vec::new()),
            evaluated: Vec::new(),
            label: Some(stored.label.clone()),
            switches: Vec::new(),
            moved: stored.moved,
            pure: true,
        };
        value.evaluated = value.covers().iter().collect();
        self.table.make(value)
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
                refs::identity(self.tys, *id)?
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
        if let Some(at) = self.table.values.of(&key) {
            return Ok(at);
        }
        if self.open.contains(&key) {
            let Source::Node(id) = source else {
                unreachable!("a formula reads no node");
            };
            return Err(refs::cyclic(self.tys, id));
        }
        self.open.push(key);
        self.table.built += 1;
        let built = self.building(key, &source, grid, name);
        self.open.pop();
        Ok(self.table.make(built?))
    }

    fn building(
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
            whole: support,
            silent: None,
            period: None,
            kind: Kind::Istft,
            reads: Vec::new(),
            held: Held::Segments(Vec::new()),
            evaluated: Vec::new(),
            label: None,
            switches: Vec::new(),
            moved: 0.0,
            pure: true,
        };
        let none = Ok;
        let Source::Node(id) = source else {
            let Source::Formula(form) = source else {
                unreachable!("a node or a formula");
            };
            let sum = sva_formula::normalize_closed_form(form).ok();
            return none(self.formula(value, sum, Some(form))?);
        };
        let id = *id;
        match (tys.ty(id).held, tys.value(id)) {
            (_, Typed::Stored(held)) => {
                value.moved = held.moved;
                value.label = Some(held.label.clone());
                value.kind = Kind::Resident(Arc::clone(held));
                none(value)
            }
            (Representation::Frames, Typed::Cast(Cast::Stft { window, hop }, of)) => {
                value.kind = Kind::Frames {
                    window: *window,
                    hop: *hop,
                };
                value.reads = vec![self.node(*of)?];
                value.held = Held::Frames(None);
                value.whole = self.support(*of);
                match value.support().is_bounded() {
                    true => Ok(value),
                    false => Err(unbounded(&value.name)),
                }
            }
            (_, Typed::Cast(Cast::Istft, frames)) => {
                value.reads = vec![self.node(*frames)?];
                Ok(value)
            }
            (_, Typed::ClosedForm(form)) if form.var == Var::F => none(self.spectrum(value, id)?),
            (_, Typed::Op { name, .. }) if tys.var(id) == Var::F => {
                Err(refs::across(tys, id, name))
            }
            (_, Typed::Cast(Cast::Fourier | Cast::IFourier, _)) if tys.var(id) == Var::F => {
                none(self.spectrum(value, id)?)
            }
            (_, Typed::ClosedForm(form)) if refs::nodes_in(&form.body).is_empty() => {
                let sum = sva_formula::normalize_closed_form(form).ok();
                none(self.formula(value, sum, Some(form))?)
            }
            (_, Typed::ClosedForm(form)) if refs::sums_through(tys, form) => {
                let sum = refs::spectral_sum_of(tys, id, Var::T)?;
                none(self.formula(value, Some(sum), None)?)
            }
            (_, Typed::Cast(Cast::Fourier | Cast::IFourier, _)) => {
                let sum = refs::spectral_sum_of(tys, id, Var::T)?;
                none(self.formula(value, Some(sum), None)?)
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
        value.kind = Kind::Spectrum(Arc::new(refs::spectral_sum_of(self.tys, id, Var::F)?));
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
        let tys = self.tys;
        let rows = whole_rate(grid).map(|rate| match (&sum, written) {
            (Some(sum), written) => refs::read_through(tys, |t| {
                Rows::of_spectral_sum_or_point(sum, written, (rate, self.profile), t)
            }),
            (None, Some(form)) => Rows::of(form, rate, self.profile),
            (None, None) => unreachable!("a formula is a sum or a written form"),
        });
        match rows {
            Some(Ok(rows)) => {
                value.width = rows.width();
                value.period = period::period(written, &rows, grid);
                value.label = Some(rows.label(self.profile));
                value.kind = Kind::Rows(Arc::new(rows));
                Ok(value)
            }
            Some(Err(e)) => Err(eval::collapse_refused(&value.name, &e)),
            None => {
                let band = sva_samples::Audible::on(self.profile, grid);
                let refused =
                    |e: &sva_samples::CollapseError| eval::collapse_refused(&value.name, e);
                let summed = sum.as_ref().map(|sum| {
                    refs::read_through(tys, |t| truncate_spectral_sum_read(sum, band, t))
                });
                let formula = match (summed, written) {
                    (Some(Ok(sum)), _) => Formula::Sum(Box::new(sum)),
                    (_, Some(form)) => Formula::Written(Box::new(Written {
                        body: truncate_written(&form.body, band).map_err(|e| refused(&e))?,
                        refs: Vec::new(),
                    })),
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
        let built = program::of(
            self.tys,
            self.supports,
            (id, value.grid),
            (self.profile, self.bounds),
        )?;
        value.moved = built.moved;
        let grid = value.grid;
        let mut reads = Vec::with_capacity(built.reads.len());
        for source in &built.reads {
            reads.push(self.source(source, grid, &value.name)?);
        }
        let endless = |slot: Slot| match slot {
            Slot::Own => true,
            Slot::Read(at) => !self.table.values[reads[at.0 as usize]]
                .support()
                .is_bounded(),
        };
        let renderer = built.renderer.stepwise(&endless);
        let stateful = !built.sites.is_empty() || reads_own(&renderer);
        let start = stateful.then(|| {
            self.supports
                .state_start(id, id)
                .unwrap_or(value.support().start)
        });
        value.switches = match stateful && self.fine == 1 {
            true => refs::switches(self.tys, id)?,
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
        let values = &self.table.values;
        let widths = reads.iter().map(|at| values[*at].width).collect();
        let live: Vec<Extent> = reads.iter().map(|at| values[*at].support()).collect();
        let layout = sva_samples::machine::ops::Layout {
            grid: value.grid,
            width: value.width,
            read_widths: widths,
            sites,
        };
        let from = start.map_or(value.support().start, |s| s.min(value.support().start));
        let spanned = Spanned::new(&renderer, &layout, (from, value.support().end), &live)
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
            renderer: Arc::new(renderer),
            spanned: Arc::new(spanned),
            layout: Arc::new(layout),
            start,
            machine: None,
            marks: Default::default(),
        }));
        value.reads = reads;
        Ok(value)
    }
}

/// The value `at` only moves, through every alias between.
fn aliased(values: &Values, mut at: usize) -> usize {
    while let Some((read, _)) = values[at].alias() {
        at = read;
    }
    at
}

/// A value's place in the store, read by none yet.
fn place(values: &Values, value: &Value, profile: &Profile) -> store::Place {
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
        fork: false,
        target: false,
        offered: false,
        slot: None,
        unread: leaf_reads(values, value),
        reached: 0,
        looked: false,
        prefixed: false,
        noted: None,
    }
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
fn leaf_reads(values: &Values, value: &Value) -> Vec<store::Unread> {
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

/// What sets a stored value's key apart from its live value's.
const PREFIX: u64 = 0x70_72_65_66_69_78_00_01;

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
