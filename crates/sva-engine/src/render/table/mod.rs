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
    Buffer, Extent, Formula, Grid, Label, NodeRenderer, Profile, Rows, Slot, Spanned,
};

pub(crate) use demand::Need;
pub(crate) use value::{Held, Key, Kind, Value};
pub(crate) use values::Values;

use crate::cache::{Memory, Question, Recording, Shape, Stored};
use crate::cast::Cast;
use crate::error::EngineError;
use crate::refs;
use crate::time::Lattice;
use crate::typing::{Typing, Value as Typed};
use program::Source;
use segments::Segments;
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
    pub(crate) built: usize,
    pub(crate) supports: Memo,
    rooted: bool,
    /// Each value started silent since the table last settled.
    silenced: Vec<usize>,
    /// What a build made and named, until settled or let go.
    draft: Draft,
    slotted: BTreeMap<NodeId, Hash>,
}

#[derive(Default)]
struct Draft {
    made: Vec<usize>,
    noded: Vec<(NodeId, Option<usize>)>,
}

/// How much finer than its own step each value is, whether a node a reader may take as its
/// samples alone is a value of its own, and the nodes memory answers.
type Bounds<'b> = (i128, bool, &'b BTreeMap<NodeId, Arc<Stored>>);

impl Table {
    pub(crate) fn new(profile: &Profile) -> Table {
        Table {
            values: Values::default(),
            nodes: BTreeMap::new(),
            root: 0,
            wanted: Vec::new(),
            planned: Vec::new(),
            profile: *profile,
            built: 0,
            supports: Memo::default(),
            rooted: false,
            silenced: Vec::new(),
            draft: Draft::default(),
            slotted: BTreeMap::new(),
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
        let none = (1, false, &BTreeMap::new());
        Table::new(profile).built(tys, (root, wanted), none, false)
    }

    /// The same, each node a reader may take as its samples alone a value of its own wherever
    /// it is read, never inlined, over the supports `found` for its range.
    pub(crate) fn bounded(
        tys: &Typing,
        (root, wanted): (NodeId, &[NodeId]),
        profile: &Profile,
        (found, hits): (Memo, &BTreeMap<NodeId, Arc<Stored>>),
    ) -> Result<Table, EngineError> {
        let mut table = Table::new(profile);
        table.supports = found;
        table.built(tys, (root, wanted), (1, true, hits), false)
    }

    /// Every read its own value, as if each were written out where it is read.
    #[cfg(test)]
    pub(crate) fn apart(
        tys: &Typing,
        root: NodeId,
        wanted: &[NodeId],
        profile: &Profile,
    ) -> Result<Table, EngineError> {
        let none = (1, false, &BTreeMap::new());
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
        let finer = (fine, false, &BTreeMap::new());
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
            held.push(self.grown(tys, Start::Node(*id), (bounds, apart))?);
        }
        self.draft = Draft::default();
        self.held_as(held[0], held[1..].to_vec());
        Ok(self)
    }

    /// The value for `id` and each it reads the table lacks, built as `bounded` builds them,
    /// each node in `hits` standing on the samples memory answered it with.
    pub(crate) fn grow(
        &mut self,
        tys: &Typing,
        id: NodeId,
        hits: &BTreeMap<NodeId, Arc<Stored>>,
    ) -> Result<usize, EngineError> {
        self.built = 0;
        let bounds = (1, true, hits);
        self.grown(tys, Start::Node(id), (bounds, false))
    }

    /// Each value standing on memory's samples, reading nothing yet, that `window` of `root`
    /// asks `past` them.
    pub(crate) fn short(&self, (root, window, past): (usize, Extent, Past)) -> Vec<usize> {
        let lazy =
            |value: &Value| matches!(value.kind, Kind::Resident(_)) && value.reads.is_empty();
        if !self.values.iter().any(|(_, value)| lazy(value)) {
            return Vec::new();
        }
        let mut asked = self.asked(window);
        asked[0].0 = root;
        let needs = demand::demand(&self.values, &asked);
        let past = |at: usize, value: &Value| match past {
            Past::Held => !needs[at].compute.is_empty(),
            Past::Stored => {
                self.draft.made.contains(&at) && !needs[at].hold.minus(&value.covers()).is_empty()
            }
        };
        let values = self.values.iter();
        values
            .filter(|(at, value)| lazy(value) && past(*at, value))
            .map(|(at, _)| at)
            .collect()
    }

    pub(crate) fn read_on(&mut self, tys: &Typing, at: usize) -> Result<(), EngineError> {
        let mut id = self.values[at].node.expect("a node memory answered");
        while let Some(source) = refs::passes(tys, id) {
            id = source;
        }
        let none = (1, true, &BTreeMap::new());
        let live = self.grown(tys, Start::Own(id), (none, false))?;
        self.values.hold(live);
        self.values[at].reads = vec![live];
        self.values.behind(at);
        let unread = leaf_reads(&self.values, &self.values[at]);
        self.values.place_mut(at).unread = unread;
        Ok(())
    }

    pub(crate) fn landed(&self, at: usize) -> i64 {
        self.values.place(at).landed
    }

    fn grown(
        &mut self,
        tys: &Typing,
        start: Start,
        ((fine, alone, hits), apart): (Bounds<'_>, bool),
    ) -> Result<usize, EngineError> {
        let memo = std::mem::take(&mut self.supports);
        let profile = self.profile;
        let supports = Supports::over(tys, Some(&memo));
        let mut building = Building {
            tys,
            supports: &supports,
            profile: &profile,
            fine,
            alone,
            hits,
            apart,
            reading: Vec::new(),
            open: Vec::new(),
            table: self,
        };
        let built = building.node(start);
        let more = supports.into_memo();
        self.supports = memo;
        self.supports.extend(more);
        built
    }

    /// `root` and `wanted` held in place of what was.
    fn held_as(&mut self, root: usize, wanted: Vec<usize>) {
        let (old, before) = (self.root, std::mem::take(&mut self.wanted));
        let had = std::mem::replace(&mut self.rooted, true);
        for at in std::iter::once(root).chain(wanted.iter().copied()) {
            self.values.hold(at);
        }
        (self.root, self.wanted) = (root, wanted);
        if had {
            let dropped: Vec<usize> = std::iter::once(old).chain(before).collect();
            let gone = self.unheld(dropped);
            self.freed(gone);
        }
    }

    /// Each value a volatile parameter reaches, built now or later, keeps its last entry alone.
    pub(crate) fn slots(&mut self, slots: &BTreeMap<NodeId, Hash>) {
        self.slotted.clone_from(slots);
        for at in self.values.ordered().collect::<Vec<_>>() {
            let held = self.values[at].node.and_then(|id| slots.get(&id).copied());
            self.values.place_mut(at).slot = held;
        }
    }

    /// Prices every value over `range` before a sample is computed; a stored one costs what
    /// computing it did.
    pub(crate) fn plan(&mut self, range: Extent) -> Result<(), EngineError> {
        let needs = self.bounded_demand(range)?;
        self.planned = self.price(&needs);
        self.stored_prices(self.values.ordered().collect());
        Ok(())
    }

    /// `plan`'s prices for `made`; one asked without end costs nothing.
    pub(crate) fn priced(&mut self, range: Extent, made: &[usize]) {
        let needs = self.demand(range);
        let made: Vec<usize> = made
            .iter()
            .copied()
            .filter(|at| self.values.holds(*at))
            .collect();
        for at in &made {
            let (value, compute) = (&self.values[*at], &needs[*at].compute);
            let bounded = compute.is_empty() || compute.hull().is_bounded();
            let price = bounded.then(|| eval::price(value, compute, &self.values));
            self.planned[*at] = price.unwrap_or(0);
        }
        self.stored_prices(made);
    }

    fn stored_prices(&mut self, ats: Vec<usize>) {
        for at in ats {
            if let Kind::Resident(stored) = &self.values[at].kind {
                self.planned[at] += stored.priced;
            }
        }
    }

    /// The most seconds any read was moved to a whole sample.
    pub(crate) fn moved(&self) -> f64 {
        let moved = self.values.iter().map(|(_, value)| value.moved);
        moved.fold(0.0, f64::max)
    }

    pub(crate) fn of(&self, node: NodeId) -> Option<usize> {
        self.nodes.get(&node).copied()
    }

    /// What a window of the root, and of each wanted node, asks of every value.
    pub(crate) fn demand(&self, window: Extent) -> Vec<Need> {
        demand::demand(&self.values, &self.asked(window))
    }

    /// What a window asks, refused where a value would compute samples without end.
    fn bounded_demand(&self, window: Extent) -> Result<Vec<Need>, EngineError> {
        let needs = self.demand(window);
        self.endless(&needs)?;
        Ok(needs)
    }

    fn endless(&self, needs: &[Need]) -> Result<(), EngineError> {
        let endless = self.values.iter().find(|(at, _)| {
            let asked = needs[*at].compute.hull();
            !asked.is_empty() && !asked.is_bounded()
        });
        match endless {
            Some((_, value)) => Err(unbounded_read(&value.name)),
            None => Ok(()),
        }
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
        (memory, seen): (&Memory, &mut Recording),
    ) -> Result<Pulled, EngineError> {
        self.pulled(&self.asked(window), (memory, seen))
    }

    /// The history each stateful value runs through before what `window` holds of it, pulled
    /// `block` samples at a time, each block dropped once no later one reads it: the state a
    /// late window starts from, streamed as a render from its start streams it.
    pub(crate) fn history(
        &mut self,
        window: Extent,
        block: i64,
        (memory, seen): (&Memory, &mut Recording),
    ) -> Result<Pulled, EngineError> {
        let needs = self.bounded_demand(window)?;
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
            let done = self.pulled(&within(Extent::new(from, to)), (memory, &mut *seen))?;
            pulled.priced += done.priced;
            pulled.waves += done.waves;
            pulled.most_bytes = pulled.most_bytes.max(done.most_bytes);
            let mut later = within(Extent::new(to, i64::MAX));
            later.extend(self.asked(window));
            let asked = (
                demand::demand(&self.values, &later),
                demand::reach(&self.values, &later),
            );
            self.released(asked, Extent::NOWHERE, Some(window.start));
            from = to;
        }
        Ok(pulled)
    }

    /// Silences each stateful value `window` asks from a sample its run has not reached, from
    /// that sample, readers first: nothing before it is computed. Those it silenced.
    pub(crate) fn skipped(&mut self, window: Extent) -> Result<Vec<usize>, EngineError> {
        let mut silenced = Vec::new();
        loop {
            let needs = self.bounded_demand(window)?;
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
        (memory, seen): (&Memory, &mut Recording),
    ) -> Result<Pulled, EngineError> {
        let mut needs = demand::demand(&self.values, asked);
        let order: Vec<usize> = self.values.ordered().collect();
        loop {
            let mut loaded = false;
            for at in order.iter().copied() {
                let (value, place) = self.values.placed(at);
                let stored = matches!(value.kind, Kind::Resident(_));
                if needs[at].hold.is_empty() || (place.looked && !stored) || !value.pure {
                    continue;
                }
                loaded |= store::load(value, place, &needs[at].hold, (memory, &mut *seen));
            }
            if !loaded {
                break;
            }
            needs = demand::demand(&self.values, asked);
        }
        self.endless(&needs)?;
        let mut pulled = Pulled::default();
        for at in order {
            let need = &needs[at];
            if need.hold.is_empty() && need.compute.is_empty() {
                continue;
            }
            if self.values[at].alias().is_some() {
                self.kept(at, &[], (memory, &mut *seen));
                continue;
            }
            let mut lifted = self.values.lift(at);
            let computed = self.computed(at, &mut lifted, need, (memory, &mut *seen));
            self.values.put(at, lifted);
            let (priced, waves) = computed?;
            let computed: Vec<Extent> = need.compute.iter().collect();
            self.kept(at, &computed, (memory, &mut *seen));
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
        (memory, seen): (&Memory, &mut Recording),
    ) -> Result<(u128, u128), EngineError> {
        let marks = store::marks(self.values.place(at), memory);
        let done = eval::compute(value, need, (&self.values, &marks), &self.profile)?;
        let place = self.values.place_mut(at);
        for (read, count) in store::reached(value, place, &need.compute) {
            let (read, place) = self.values.placed(read);
            store::reread(read, place, count, seen);
        }
        Ok(done)
    }

    /// Drops what no later window reads: `future` is the rest of the root's range, `keep` more
    /// the root holds besides, and `since` where the output is read from, if it is.
    pub(crate) fn release(&mut self, future: Option<Extent>, keep: Extent, since: Option<i64>) {
        let asked = match future {
            Some(window) => {
                let asked = self.asked(window);
                (self.demand(window), demand::reach(&self.values, &asked))
            }
            None => (
                vec![Need::default(); self.values.span()],
                vec![Segments::default(); self.values.span()],
            ),
        };
        self.released(asked, keep, since);
    }

    /// A wanted value keeps everything, as a wanted reader's rerun reads it from its start;
    /// the target's own value, which no wanted value reads, keeps only the output's samples.
    fn released(
        &mut self,
        (mut needs, reach): (Vec<Need>, Vec<Segments>),
        keep: Extent,
        since: Option<i64>,
    ) {
        let (mut root, mut by) = (self.root, 0);
        while let Some((read, shift)) = self.values[root].alias() {
            (root, by) = (read, by + shift);
        }
        let output = !self
            .wanted
            .iter()
            .any(|w| *w != root && self.values[*w].reads.contains(&root));
        let wholes = self.wholes();
        for at in self.values.ordered().collect::<Vec<_>>() {
            let need = std::mem::take(&mut needs[at]);
            let whole = wholes.contains(&at) && !(output && at == root);
            let value = &self.values[at];
            if value.alias().is_some() || whole {
                continue;
            }
            let mut kept = need.hold;
            if at == root {
                kept.add(keep.shifted(by));
                if let Some(since) = since.filter(|_| wholes.contains(&at)) {
                    kept.add(Extent::new(since, i64::MAX).shifted(by));
                }
            }
            let value = &mut self.values[at];
            if let (Kind::Program(program), Some(end)) = (&value.kind, value.end()) {
                kept.add(Extent::new(end.saturating_sub(program.own), end));
                kept.union(&reach[at].intersect(Extent::new(i64::MIN, end)));
            }
            value.retain(&kept);
        }
    }

    /// Each wanted value, and each one a wanted value only moves: held over the whole range.
    fn wholes(&self) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        for w in &self.wanted {
            let mut v = *w;
            while out.insert(v)
                && let Some((read, _)) = self.values[v].alias()
            {
                v = read;
            }
        }
        out
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
            out[at] = eval::price(value, &needs[at].compute, &self.values);
        }
        out
    }

    /// A value made, holding what it reads.
    fn make(&mut self, value: Value) -> usize {
        let mut place = place(&self.values, &value, &self.profile);
        place.slot = value.node.and_then(|id| self.slotted.get(&id).copied());
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
        (self.root, self.rooted) = (root, true);
        let going = self.unheld(dropped);
        let mut made: Vec<usize> = draft
            .made
            .into_iter()
            .filter(|at| self.values.held(*at))
            .collect();
        made.sort_by_key(|at| self.values.seq(*at));
        for at in &made {
            self.values.place_mut(*at).landed = now;
        }
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
    alone: bool,
    hits: &'a BTreeMap<NodeId, Arc<Stored>>,
    /// Every read its own value rather than one per identity.
    apart: bool,
    /// The nodes whose reads are being built, apart.
    reading: Vec<NodeId>,
    open: Vec<Key>,
    table: &'a mut Table,
}

/// A step of a build, run off a stack so that a chain of reads costs heap, never call depth;
/// each step resolving a value leaves its slot on the resolved stack.
enum Step {
    Node(NodeId),
    /// Names a node passing another's value the value just resolved.
    Pass(NodeId),
    /// Names a node the value just built for it.
    Own(NodeId),
    Source(Source, Grid, String),
    /// Ends reading a node apart.
    Read,
    Value(Key, Source, Grid, String),
    /// Makes a value once each value it reads is resolved.
    Finish(Box<Open>),
}

struct Open {
    value: Value,
    then: Then,
    reads: usize,
}

/// What a value standing on memory's samples is opened past: what it holds now, or, for one
/// the latest build made, all memory holds of it.
#[derive(Clone, Copy)]
pub(crate) enum Past {
    Held,
    Stored,
}

/// Where a build starts: a node, or a node's own value, never the samples memory answered.
#[derive(Clone, Copy)]
enum Start {
    Node(NodeId),
    Own(NodeId),
}

/// What completes a value once its reads are resolved.
enum Then {
    Done,
    /// A short-time transform, refused where its input never ends.
    Frames,
    Program(NodeId, Box<program::Program>),
}

impl Building<'_> {
    /// `start`'s value, each value it reads built before it.
    fn node(&mut self, start: Start) -> Result<usize, EngineError> {
        let mut steps = match start {
            Start::Node(id) => vec![Step::Node(id)],
            Start::Own(id) => {
                let (key, grid, name) = self.own(id)?;
                vec![Step::Value(key, Source::Node(id), grid, name)]
            }
        };
        let mut resolved: Vec<usize> = Vec::new();
        while let Some(next) = steps.pop() {
            match next {
                Step::Node(id) => self.noded(id, &mut steps, &mut resolved)?,
                Step::Pass(id) => {
                    let at = *resolved.last().expect("the passed value");
                    self.table.name(id, at);
                }
                Step::Own(id) => {
                    let at = *resolved.last().expect("the node's value");
                    self.table.name(id, at);
                }
                Step::Source(source, grid, name) => {
                    steps.extend(self.source(source, grid, name)?);
                }
                Step::Read => {
                    self.reading.pop();
                }
                Step::Value(key, source, grid, name) => {
                    if let Some(at) = self.table.values.of(&key).filter(|_| !self.apart) {
                        resolved.push(at);
                        continue;
                    }
                    if self.open.contains(&key) && !self.apart {
                        let Source::Node(id) = source else {
                            unreachable!("a formula reads no node");
                        };
                        return Err(refs::cyclic(self.tys, id));
                    }
                    self.open.push(key);
                    self.table.built += 1;
                    let (value, then, reads) = self.building(key, &source, grid, &name)?;
                    let open = Open {
                        value,
                        then,
                        reads: reads.len(),
                    };
                    steps.push(Step::Finish(Box::new(open)));
                    steps.extend(reads.into_iter().rev());
                }
                Step::Finish(open) => {
                    let reads = resolved.split_off(resolved.len() - open.reads);
                    let value = self.finished(*open, reads)?;
                    self.open.pop();
                    resolved.push(self.table.make(value));
                }
            }
        }
        Ok(resolved.pop().expect("the node's value"))
    }

    fn noded(
        &mut self,
        id: NodeId,
        steps: &mut Vec<Step>,
        resolved: &mut Vec<usize>,
    ) -> Result<(), EngineError> {
        if let Some(at) = self.table.nodes.get(&id) {
            resolved.push(*at);
            return Ok(());
        }
        let grid = self.grid(id);
        let hit = self.hits.get(&id).filter(|stored| {
            stored.grid == grid && usize::from(stored.width) == width(self.tys, id)
        });
        if let Some(stored) = hit {
            let at = self.resident(id, self.own(id)?, stored);
            self.table.name(id, at);
            resolved.push(at);
            return Ok(());
        }
        if let Some(source) = refs::passes(self.tys, id) {
            steps.extend([Step::Pass(id), Step::Node(source)]);
            return Ok(());
        }
        let (key, grid, name) = self.own(id)?;
        steps.extend([
            Step::Own(id),
            Step::Value(key, Source::Node(id), grid, name),
        ]);
        Ok(())
    }

    fn own(&self, id: NodeId) -> Result<(Key, Grid, String), EngineError> {
        let grid = self.grid(id);
        let key = Key {
            identity: refs::identity(self.tys, id)?,
            step: step(grid),
        };
        Ok((key, grid, self.tys.name(id).to_string()))
    }

    /// The samples memory answered node `id` with, reading nothing until a reader asks past
    /// them; its own value's key set apart.
    fn resident(
        &mut self,
        id: NodeId,
        (key, grid, name): (Key, Grid, String),
        stored: &Arc<Stored>,
    ) -> usize {
        let key = Key {
            identity: crate::cache::mixed(key.identity, &[PREFIX]),
            step: key.step,
        };
        if let Some(at) = self.table.values.of(&key) {
            return at;
        }
        let value = Value {
            key,
            node: Some(id),
            name,
            grid,
            width: usize::from(stored.width),
            whole: self.support(id),
            silent: None,
            period: None,
            kind: Kind::Resident(Arc::clone(stored)),
            reads: Vec::new(),
            held: Held::Segments(Vec::new()),
            evaluated: Vec::new(),
            label: Some(stored.label.clone()),
            switches: Vec::new(),
            moved: stored.moved,
            pure: true,
        };
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

    /// The steps resolving a read, last first: apart, a value of its own for this read.
    fn source(
        &mut self,
        source: Source,
        grid: Grid,
        name: String,
    ) -> Result<Vec<Step>, EngineError> {
        let identity = match (&source, self.apart) {
            (Source::Node(id), false) => return Ok(vec![Step::Node(*id)]),
            (Source::Node(id), true) if let Some(passed) = refs::passes(self.tys, *id) => {
                return Ok(vec![Step::Source(Source::Node(passed), grid, name)]);
            }
            (Source::Node(id), true) => {
                if self.reading.contains(id) {
                    return Err(refs::cyclic(self.tys, *id));
                }
                refs::identity(self.tys, *id)?
            }
            (Source::Formula(form), _) => refs::subterm_identity(self.tys, form)?,
        };
        let Source::Node(id) = source else {
            let key = Key {
                identity,
                step: step(grid),
            };
            return Ok(vec![Step::Value(key, source, grid, name)]);
        };
        let grid = self.grid(id);
        let key = Key {
            identity,
            step: step(grid),
        };
        self.reading.push(id);
        let name = self.tys.name(id).to_string();
        Ok(vec![Step::Read, Step::Value(key, source, grid, name)])
    }

    /// A value before its reads are resolved, what completes it, and the steps resolving them.
    fn building(
        &mut self,
        key: Key,
        source: &Source,
        grid: Grid,
        name: &str,
    ) -> Result<(Value, Then, Vec<Step>), EngineError> {
        let tys = self.tys;
        let (node, support, width) = match source {
            Source::Node(id) => (Some(*id), self.support(*id), width(tys, *id)),
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
        let none = |value| Ok((value, Then::Done, Vec::new()));
        let Source::Node(id) = source else {
            let Source::Formula(form) = source else {
                unreachable!("a node or a formula");
            };
            return none(self.written(value, form)?);
        };
        let id = *id;
        match (tys.ty(id).held, tys.value(id)) {
            (Representation::Frames, Typed::Cast(Cast::Stft { window, hop }, of)) => {
                value.kind = Kind::Frames {
                    window: *window,
                    hop: *hop,
                };
                value.held = Held::Frames(None);
                value.whole = self.support(*of);
                Ok((value, Then::Frames, vec![Step::Node(*of)]))
            }
            (_, Typed::Cast(Cast::Istft, frames)) => {
                Ok((value, Then::Done, vec![Step::Node(*frames)]))
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
            (_, Typed::ClosedForm(form))
                if form.var == Var::T && program::is_one_value(tys, &form.body) =>
            {
                none(self.written(value, form)?)
            }
            _ => self.program(value, id),
        }
    }

    /// A closed form written in a body: its spectral sum where it has one, else its form, each
    /// ref it reads read as the form it names.
    fn written(&mut self, value: Value, form: &ClosedForm) -> Result<Value, EngineError> {
        let sum = match refs::nodes_in(&form.body).is_empty() {
            true => sva_formula::normalize_closed_form(form).ok(),
            false => refs::read_through(self.tys, |t| {
                sva_formula::normalize_read(&form.body, form.var, t).ok()
            }),
        };
        self.formula(value, sum, Some(form))
    }

    /// A form in `f` is, on the grid, the form in `t` its dual is; one with no dual refuses as
    /// the table refuses its dual.
    fn spectrum(&mut self, value: Value, id: NodeId) -> Result<Value, EngineError> {
        let sum = refs::spectral_sum_of(self.tys, id, Var::T)?;
        self.formula(value, Some(sum), None)
    }

    /// Its rows, else its written form, which reads refs, at each instant.
    fn formula(
        &mut self,
        mut value: Value,
        sum: Option<sva_formula::SpectralSum>,
        written: Option<&ClosedForm>,
    ) -> Result<Value, EngineError> {
        let grid = value.grid;
        let tys = self.tys;
        let reads = written.is_some_and(|form| !refs::nodes_in(&form.body).is_empty());
        let free = written.filter(|_| !reads);
        let rows = rows(tys, (sum.as_ref(), free), grid, self.profile);
        let refused = |e: &sva_samples::CollapseError| eval::collapse_refused(&value.name, e);
        let form = match (rows, written.filter(|_| reads)) {
            (Some(Ok(rows)), _) => {
                value.width = rows.width();
                value.period = period::period(free, &rows, grid);
                value.label = Some(rows.label(self.profile));
                value.kind = Kind::Rows(Arc::new(rows));
                return Ok(value);
            }
            (Some(Err(e)), None) => return Err(refused(&e)),
            (_, Some(form)) => form,
            (None, None) => unreachable!("a formula is a sum or a written form"),
        };
        let band = sva_samples::Audible::on(self.profile, grid);
        let shared = program::shared(tys, &form.body, band).map_err(|e| refused(&e))?;
        let renderer = NodeRenderer::Formula {
            formula: Formula::Written(Box::new(shared)),
            width: value.width,
            time: Box::new(NodeRenderer::Time),
        };
        if let Some(tail_db) = dropped_db(&renderer) {
            value.label = Some(Label::new(
                sva_samples::Source::Measured,
                self.profile.name,
                grid.rate,
                sva_samples::Detail::Point {
                    rule: sva_samples::Rule::PointSampled,
                    alias_db: None,
                    tail_db: Some(tail_db),
                },
            ));
        }
        self.running(value, renderer, (Vec::new(), Vec::new()), Vec::new(), None)
    }

    /// A closed form's program point-samples it; any other program is a reading of samples.
    fn program(
        &mut self,
        mut value: Value,
        id: NodeId,
    ) -> Result<(Value, Then, Vec<Step>), EngineError> {
        let built = program::of(
            self.tys,
            self.supports,
            (id, value.grid),
            (self.profile, self.alone),
        )?;
        if self.tys.ty(id).is_closed_form() {
            value.label = Some(Label::new(
                sva_samples::Source::Measured,
                self.profile.name,
                value.grid.rate,
                sva_samples::Detail::Point {
                    rule: sva_samples::Rule::PointSampled,
                    alias_db: None,
                    tail_db: dropped_db(&built.renderer),
                },
            ));
        }
        value.moved = built.moved;
        let reads = built
            .reads
            .iter()
            .map(|source| Step::Source(source.clone(), value.grid, value.name.clone()))
            .collect();
        Ok((value, Then::Program(id, Box::new(built)), reads))
    }

    fn finished(&mut self, open: Open, reads: Vec<usize>) -> Result<Value, EngineError> {
        let Open {
            mut value, then, ..
        } = open;
        match then {
            Then::Done => {
                value.reads = reads;
                Ok(value)
            }
            Then::Frames => {
                value.reads = reads;
                match value.support().is_bounded() {
                    true => Ok(value),
                    false => Err(unbounded(&value.name)),
                }
            }
            Then::Program(id, built) => self.programmed(value, id, *built, reads),
        }
    }

    fn programmed(
        &mut self,
        mut value: Value,
        id: NodeId,
        built: program::Program,
        reads: Vec<usize>,
    ) -> Result<Value, EngineError> {
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
        let sources = built.reads.iter().map(|source| match source {
            Source::Node(id) => Some(*id),
            Source::Formula(_) => None,
        });
        let reads = (reads, sources.collect());
        self.running(value, renderer, reads, built.sites, start)
    }

    fn running(
        &mut self,
        mut value: Value,
        renderer: NodeRenderer,
        (reads, sources): (Vec<usize>, Vec<Option<NodeId>>),
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
            value.held = Held::Run {
                samples: Buffer::empty(value.grid.rate, value.width, 0, from),
                origin: from,
            };
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
            sources: sources.into(),
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

fn width(tys: &Typing, id: NodeId) -> usize {
    usize::from(tys.ty(id).width).max(1)
}

/// Samples, and nothing reads it between them.
pub(crate) fn readable(tys: &Typing, id: NodeId) -> bool {
    tys.ty(id).held == Representation::Sampled && !crate::schedule::anywhere(tys, id)
}

/// What memory holds node `id`'s own value under, its identity `identity`.
pub(crate) fn node_key(tys: &Typing, id: NodeId, identity: Hash, profile: &Profile) -> Hash {
    let question = Question {
        grid: tys.grid(id),
        width: width(tys, id),
        profile,
        shape: Shape::Samples,
    };
    crate::cache::key(identity, &question)
}

/// A value's place in the store, read by none yet.
fn place(values: &Values, value: &Value, profile: &Profile) -> store::Place {
    let shape = match value.kind {
        Kind::Frames { window, hop } => Shape::Frames { window, hop },
        _ => Shape::Samples,
    };
    let question = Question {
        grid: value.grid,
        width: value.width,
        profile,
        shape,
    };
    let key = |identity| crate::cache::key(identity, &question);
    store::Place {
        key: key(value.key.identity),
        segments: segments(&value.switches, key(value.key.identity), key),
        slot: None,
        unread: leaf_reads(values, value),
        reached: 0,
        looked: false,
        offer: None,
        told: false,
        landed: i64::MIN,
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

/// The loudest term any series a program's formulas sum instant by instant may drop.
fn dropped_db(renderer: &NodeRenderer) -> Option<f64> {
    let mut held: Option<f64> = None;
    program::leaves(renderer, &mut |leaf| {
        if let NodeRenderer::Formula {
            formula: Formula::Written(written),
            ..
        } = leaf
        {
            let found = std::iter::once(&written.body).chain(&written.refs);
            for db in found.filter_map(sva_samples::dropped_db) {
                held = Some(held.map_or(db, |held| held.max(db)));
            }
        }
    });
    held
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

/// A value asked for every sample of a support that never starts or ends.
fn unbounded_read(name: &str) -> EngineError {
    EngineError::refused(crate::error::Diagnostic {
        code: "engine.unbounded_read".to_string(),
        message: format!(
            "`{name}` is asked for every one of its samples, and it never starts or ends"
        ),
        location: crate::error::Located::at(name, None),
        help: format!(
            "crop `{name}` to a window, or bound how far an index reading it reaches, as `t` \
             plus a bounded offset does"
        ),
    })
}

/// What sets a stored value's key apart from its live value's.
const PREFIX: u64 = 0x70_72_65_66_69_78_00_01;

fn step(grid: Grid) -> (i128, i128) {
    (grid.a, grid.d)
}

/// A closed form's rows on `grid`, which its samples and every warp of it read; `None` for a
/// written form reading refs with no sum.
pub(super) fn rows(
    tys: &Typing,
    (sum, free): (Option<&sva_formula::SpectralSum>, Option<&ClosedForm>),
    grid: Grid,
    profile: &Profile,
) -> Option<Result<Rows, sva_samples::CollapseError>> {
    let on = whole_rate(grid).map_or(grid, Grid::of);
    match (sum, free) {
        (Some(sum), free) => Some(refs::read_through(tys, |t| {
            Rows::of_spectral_sum_or_point(sum, free, (on, profile), t)
        })),
        (None, Some(form)) => Some(Rows::of(form, on, profile)),
        (None, None) => None,
    }
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
