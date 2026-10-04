// Concern: what memory answers a value with before it computes, and what it keeps after, with its node | Non-concern: memory's cap and evictions | IO: (value) -> samples, needs; (value) -> kept

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use sva_formula::{Hash, Held as Representation, NodeId};
use sva_samples::{Buffer, Extent, Machine, MachineState, NodeRenderer, Tape};

use super::Table;
use super::segments::Segments;
use super::value::{Held, Kind, Value};
use crate::cache::{
    Expected, Facts, Keep, Offered, Outcome, Payload, PayloadKind, Recording, Run, Stored,
};
use crate::typing::Typing;

/// Where a value sits in memory: its key, and the slot a volatile parameter gives it.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    pub(crate) key: Hash,
    /// A run cut where its switches turn, each segment under the value's identity before the
    /// next switch, the last under its own; the first starts where its run does.
    pub(crate) segments: Vec<(i64, Hash)>,
    pub(crate) slot: Option<Hash>,
    /// Its own reads its samples have not yet run.
    pub(crate) unread: Vec<Unread>,
    /// Reads of it run so far.
    pub(crate) reached: usize,
    pub(crate) looked: bool,
    /// Answered only up to a switch.
    pub(crate) prefixed: bool,
    /// The lookup its first ask made, once made.
    pub(crate) noted: Option<usize>,
    /// The node its samples answer, where it is an instance's own.
    pub(crate) offer: Option<Offer>,
    /// Whether memory was told of that node.
    pub(crate) told: bool,
    /// The sample the change that made it landed at.
    pub(crate) landed: i64,
}

/// The node a value's samples answer, as the table named it.
#[derive(Clone, Debug)]
pub(crate) struct Offer {
    stored: Stored,
    /// Where its samples are; `None` where it copies them out of a period, over `over`.
    offered: Option<Offered>,
    over: Extent,
    facts: Facts,
}

/// `count` leaves reading value `read`; no leaf is a read run with the reader's first samples.
#[derive(Clone, Debug)]
pub(crate) struct Unread {
    pub(crate) leaf: Option<NodeRenderer>,
    pub(crate) read: usize,
    pub(crate) count: usize,
}

impl Place {
    fn kind(value: &Value) -> PayloadKind {
        match (&value.kind, &value.held) {
            (Kind::Frames { .. }, _) => PayloadKind::Frames,
            (_, Held::Run(_)) => PayloadKind::Run,
            _ => PayloadKind::Segments,
        }
    }

    fn end(&self, k: usize) -> i64 {
        self.segments
            .get(k + 1)
            .map_or(i64::MAX, |(start, _)| *start)
    }

    fn parent(&self, k: usize) -> Option<Hash> {
        k.checked_sub(1).map(|p| self.segments[p].1)
    }
}

/// What memory holds of `value`, laid in beside what it holds itself; `false` where memory
/// answered nothing. A node memory answered takes what `hold` asks that it lacks, each time.
pub(crate) fn load(
    value: &mut Value,
    place: &mut Place,
    hold: &Segments,
    recording: &Recording,
) -> bool {
    if let Kind::Resident(stored) = &value.kind {
        let (stored_over, lacks) = (value.covers(), hold.minus(&value.holding()));
        let lacks = stored_over.minus(&stored_over.minus(&lacks));
        if lacks.is_empty() {
            return false;
        }
        let parts = recording.resident(stored.key, lacks.hull());
        return laid(value, &parts);
    }
    place.looked = true;
    let kind = Place::kind(value);
    let (rate, width) = (value.grid.rate, value.width);
    if kind == PayloadKind::Run {
        return resumed(value, place, recording);
    }
    let expected = match kind {
        PayloadKind::Frames => Expected::Frames,
        _ => Expected::Segments { rate, width },
    };
    let Some(entry) = recording.load(place.key, expected) else {
        return false;
    };
    if entry.label.is_some() {
        value.label = entry.label;
    }
    match entry.payload {
        Payload::Segments(parts) => {
            for part in parts {
                value.hold_shared(part);
            }
            true
        }
        Payload::Frames(frames) => {
            value.held = Held::Frames(Some(frames));
            true
        }
        Payload::Run(_) => false,
    }
}

/// Segment by segment from the first: a segment's samples up to the next switch, on to the
/// next segment where its state there is marked, else from its last mark. A value that has
/// stepped already takes none.
fn resumed(value: &mut Value, place: &mut Place, recording: &Recording) -> bool {
    let Kind::Program(program) = &value.kind else {
        return false;
    };
    if program.machine.is_some() {
        return false;
    }
    let expected = Expected::Run {
        rate: value.grid.rate,
        width: value.width,
    };
    let mut planes = vec![Vec::new(); value.width];
    let (mut base, mut pos, mut reached) = (None, i64::MIN, 0);
    let mut marks: BTreeMap<i64, MachineState> = BTreeMap::new();
    for k in 0..place.segments.len() {
        let Some(run) = recording
            .load(place.segments[k].1, expected)
            .and_then(|entry| entry.payload.run())
        else {
            break;
        };
        let placed = match k {
            0 => true,
            _ => run.samples.start == pos && run.parent == place.parent(k),
        };
        if !placed {
            break;
        }
        if k == 0 {
            (base, pos) = (Some(run.samples.start), run.samples.start);
        }
        let upto = place.end(k).min(run.end());
        for (c, plane) in planes.iter_mut().enumerate() {
            let at = |n: i64| run.samples.plane(c)[(n - run.samples.start) as usize];
            plane.extend((pos..upto).map(at));
        }
        marks.extend(run.marks.range(pos..=upto).map(|(at, m)| (*at, m.clone())));
        (pos, reached) = (upto, k + 1);
        if upto < place.end(k) || !run.marks.contains_key(&upto) {
            break;
        }
    }
    let (Some(base), Some((&at, state))) = (base, marks.range(..=pos).next_back()) else {
        return false;
    };
    let Ok(mut machine) = Machine::over(&program.spanned, at) else {
        return false;
    };
    if at <= base || !machine.carry(state) {
        return false;
    }
    for plane in &mut planes {
        plane.truncate((at - base) as usize);
    }
    let mut samples = Buffer::of_planes(value.grid.rate, planes);
    samples.start = base;
    place.prefixed = reached < place.segments.len();
    let Kind::Program(program) = &mut value.kind else {
        unreachable!("a program");
    };
    program.machine = Some(machine);
    program.marks = marks.into_iter().filter(|(m, _)| *m <= at).collect();
    value.held = Held::Run(Tape::from(samples));
    true
}

/// The first time a value is asked: one lookup, answered by what it computes now.
pub(crate) fn noted(value: &Value, place: &mut Place, computes: bool, recording: &mut Recording) {
    if place.noted.is_some() || matches!(value.kind, Kind::Resident { .. }) {
        return;
    }
    let (kind, key) = (Place::kind(value), place.key);
    let first = match (computes, place.prefixed) {
        (_, true) => Outcome::Prefix,
        (true, false) => Outcome::ComputedNotStored,
        (false, false) => Outcome::Hit,
    };
    place.noted = Some(recording.note(&value.name, key, kind, first));
}

/// `count` more reads of a value run, each past the first a reuse.
pub(crate) fn reread(value: &Value, place: &mut Place, count: usize, recording: &mut Recording) {
    if place.noted.is_none() {
        return;
    }
    let (kind, key) = (Place::kind(value), place.key);
    for _ in 0..count {
        if place.reached > 0 {
            recording.note(&value.name, key, kind, Outcome::Hit);
        }
        place.reached += 1;
    }
}

/// The reads `computed` first runs, off `reader`'s unread list.
pub(crate) fn reached(
    reader: &Value,
    place: &mut Place,
    computed: &Segments,
) -> Vec<(usize, usize)> {
    if place.unread.is_empty() || computed.is_empty() {
        return Vec::new();
    }
    let mut runs: Vec<bool> = place.unread.iter().map(|u| u.leaf.is_none()).collect();
    if let Kind::Program(program) = &reader.kind {
        for span in program.spanned.spans() {
            let met = computed
                .iter()
                .any(|e| !e.intersect(Extent::new(span.from, span.to)).is_empty());
            if met {
                super::program::leaves(&span.renderer, &mut |leaf| {
                    for (k, unread) in place.unread.iter().enumerate() {
                        runs[k] |= unread.leaf.as_ref() == Some(leaf);
                    }
                });
            }
        }
    }
    let mut out = Vec::new();
    let mut k = 0;
    place.unread.retain(|unread| {
        let reached = runs[k];
        k += 1;
        if reached {
            out.push((unread.read, unread.count));
        }
        !reached
    });
    out
}

/// What a value computed, as memory takes it: a run segment by segment, each with the states
/// it marked, the rest under its own key.
fn samples(value: &mut Value, place: &Place, computed: &[Extent]) -> Vec<(Hash, Payload)> {
    let stored = matches!(value.kind, Kind::Resident { .. });
    if computed.is_empty() || stored || !value.pure {
        return Vec::new();
    }
    let payload = match &mut value.held {
        Held::Segments(parts) => Payload::Segments(
            computed
                .iter()
                .flat_map(|e| parts.iter().filter_map(move |b| over(b, *e)))
                .collect(),
        ),
        Held::Frames(Some(frames)) => Payload::Frames(Arc::clone(frames)),
        Held::Frames(None) => return Vec::new(),
        Held::Run(tape) => {
            let (Some(from), Kind::Program(program)) = (computed.first(), &mut value.kind) else {
                return Vec::new();
            };
            let Some(machine) = &program.machine else {
                return Vec::new();
            };
            program.marks.insert(tape.end(), machine.state());
            let marks = std::mem::take(&mut program.marks);
            let mut out = Vec::new();
            for (k, (start, segment)) in place.segments.iter().enumerate() {
                let (lo, hi) = (from.start.max(*start), tape.end().min(place.end(k)));
                if lo >= hi {
                    continue;
                }
                let piece = Extent::new(lo, hi);
                let Some(chunk) = taped(tape, value.grid.rate, piece) else {
                    continue;
                };
                let run = Run {
                    samples: Arc::new(chunk),
                    marks: marks
                        .range(piece.start..=piece.end)
                        .map(|(at, m)| (*at, m.clone()))
                        .collect(),
                    parent: place.parent(k),
                };
                out.push((*segment, Payload::Run(Arc::new(run))));
            }
            return out;
        }
    };
    vec![(place.key, payload)]
}

/// `parts` laid into a value standing on memory's samples wherever it lacks them, shared
/// wherever one falls whole within; whether any was.
fn laid(value: &mut Value, parts: &[Arc<Buffer>]) -> bool {
    let mut any = false;
    for part in parts {
        let lacks = value.covers().minus(&value.holding());
        for e in lacks.intersect(part.extent()).iter() {
            any = true;
            match e == part.extent() {
                true => value.hold_shared(Arc::clone(part)),
                false => value.hold(part.over(e, part.extent())),
            }
        }
    }
    any
}

fn over(buffer: &Arc<Buffer>, e: Extent) -> Option<Arc<Buffer>> {
    let held = buffer.extent();
    if e.is_empty() || e.start < held.start || held.end < e.end {
        return None;
    }
    Some(match e == held {
        true => Arc::clone(buffer),
        false => Arc::new(buffer.over(e, held)),
    })
}

fn taped(tape: &Tape, rate: u32, e: Extent) -> Option<Buffer> {
    if e.is_empty() || e.start < tape.base() || tape.end() < e.end {
        return None;
    }
    let len = e.len();
    let planes = (0..tape.width())
        .map(|c| tape.since(c, e.start)[..len].to_vec())
        .collect();
    let mut out = Buffer::of_planes(rate, planes);
    out.start = e.start;
    Some(out)
}

impl Table {
    /// What `at` computed, kept with the node its samples answer.
    pub(crate) fn kept(&mut self, at: usize, computed: &[Extent], recording: &mut Recording) {
        if !recording.keeps() {
            return;
        }
        let (value, place) = self.values.placed(at);
        let kept = samples(value, place, computed);
        let (slot, noted, label) = (place.slot, place.noted, value.label.clone());
        let mut node = self.node(at, !kept.is_empty());
        for (key, payload) in kept {
            let keep = Keep {
                samples: Some(payload),
                label: label.as_ref(),
                slot,
                node: node.take_if(|(stored, _, _)| stored.key == key),
            };
            recording.keep((key, noted), keep);
        }
        if let Some(node) = node {
            let key = node.0.key;
            let keep = Keep {
                samples: None,
                label: None,
                slot,
                node: Some(node),
            };
            recording.keep((key, None), keep);
        }
    }

    /// The node to tell memory of now: with each keep of its own samples, else once.
    fn node(&mut self, at: usize, keeping: bool) -> Option<(Stored, Offered, Facts)> {
        let label = self.label(at);
        let value = &self.values[at];
        let place = self.values.place(at);
        let offer = place.offer.as_ref()?;
        if !value.pure || (place.told && !(keeping && offer.offered == Some(Offered::Own))) {
            return None;
        }
        let offered = match &offer.offered {
            Some(Offered::Own) if value.holding().is_empty() => return None,
            Some(offered) => offered.clone(),
            None if !offer.over.is_bounded() || !self.copied(at, offer.over) => return None,
            None => Offered::Held(vec![Arc::new(self.samples(at, offer.over))]),
        };
        let stored = Stored {
            label,
            ..offer.stored.clone()
        };
        let facts = offer.facts;
        self.values.place_mut(at).told = true;
        Some((stored, offered, facts))
    }

    fn copied(&self, at: usize, over: Extent) -> bool {
        let (mut foot, mut by) = (at, 0);
        while let Some((read, shift)) = self.values[foot].alias() {
            (foot, by) = (read, by + shift);
        }
        let value = &self.values[foot];
        let mut asked = Segments::of(over.shifted(by)).intersect(value.support());
        if let Some(period) = value.period {
            asked = asked.folded(period);
        }
        value.holding().covers(&asked)
    }

    /// Each value an instance's own node holds named by that node, priced with all under it; a
    /// value that only moves another is answered as what it moves.
    pub(crate) fn offers(&mut self, tys: &Typing, range: Extent) {
        let under = Under::of(self);
        let mut named: BTreeMap<usize, Stored> = BTreeMap::new();
        for (id, at) in self.nodes.clone() {
            let at = match (&self.values[at].kind, &self.values[at].reads[..]) {
                (Kind::Resident(_), [live]) => *live,
                _ => at,
            };
            if named.contains_key(&at) {
                continue;
            }
            if let Some(stored) = self.offerable(tys, (id, at), &under) {
                named.insert(at, stored);
            }
        }
        let feet = self.values.feet();
        for (at, stored) in named.iter() {
            let foot = feet.get(at).copied();
            let moved = foot.and_then(|(foot, by)| {
                let of = match &self.values[foot].kind {
                    Kind::Resident(held) => held.key,
                    _ => named.get(&foot)?.key,
                };
                Some(Offered::Moves { of, by })
            });
            let offered = moved.or_else(|| {
                let (place, by) = foot.unwrap_or((*at, 0));
                let value = &self.values[place];
                match (value.period, foot, self.values.place(place).key) {
                    (Some(_), _, _) => None,
                    (None, None, own) if own == stored.key => Some(Offered::Own),
                    (None, _, of) => Some(Offered::Moves { of, by }),
                }
            });
            let over = range.intersect(stored.support);
            let samples = match over.is_bounded() {
                true => over.len() as u64,
                false => u64::MAX,
            };
            let facts = Facts {
                target: *at == self.root,
                samples,
            };
            let offer = Offer {
                stored: stored.clone(),
                offered,
                over,
                facts,
            };
            self.values.place_mut(*at).offer = Some(offer);
        }
    }

    fn offerable(&self, tys: &Typing, (id, at): (NodeId, usize), under: &Under) -> Option<Stored> {
        let value = &self.values[at];
        let path = tys.name(id);
        let own = tys.id(path) == Some(id) && crate::refs::passes(tys, id).is_none();
        let kind = matches!(value.kind, Kind::Frames { .. } | Kind::Resident(_));
        if !own || kind || !value.pure {
            return None;
        }
        let identity = crate::refs::identity(tys, id).ok()?;
        let (priced, moved) = under.of_value(at);
        let ty = tys.ty(id);
        Some(Stored {
            key: super::node_key(tys, id, identity, &self.profile),
            identity,
            label: self.label(at),
            width: u8::try_from(value.width).expect("a width the typing held"),
            codomain: ty.codomain,
            rate: ty.rate,
            grid: tys.grid(id),
            support: value.support(),
            priced,
            moved,
            readable: super::readable(tys, id) && value.alias().is_none(),
            sampled: ty.held == Representation::Sampled,
            held: Vec::new(),
        })
    }

    /// Each node memory holds that `window` asks samples of no value holds: its key, and the
    /// stretch.
    pub(crate) fn needs(&self, window: Extent) -> Vec<(Hash, Extent)> {
        if !self.lacks() {
            return Vec::new();
        }
        let needs = self.demand(window);
        let ats: Vec<usize> = self.values.ordered().collect();
        self.lacking(&ats, &needs)
    }

    pub(crate) fn needs_made(&self, root: usize, window: Extent) -> Vec<(Hash, Extent)> {
        let needs = super::demand::demand(&self.values, &[(root, window)]);
        self.lacking(self.made(), &needs)
    }

    /// Whether a node memory holds lacks samples a value standing on it may yet be asked.
    fn lacks(&self) -> bool {
        self.values.iter().any(|(_, value)| {
            matches!(value.kind, Kind::Resident(_)) && !value.holding().covers(&value.covers())
        })
    }

    fn lacking(&self, ats: &[usize], needs: &[super::Need]) -> Vec<(Hash, Extent)> {
        let mut out = Vec::new();
        for at in ats {
            let value = &self.values[*at];
            let Kind::Resident(stored) = &value.kind else {
                continue;
            };
            let lacks = needs[*at].hold.minus(&value.holding());
            for run in value.covers().iter() {
                let asked = lacks.intersect(run);
                if !asked.is_empty() {
                    out.push((stored.key, asked.hull()));
                }
            }
        }
        out
    }

    /// Samples memory handed out for `key`, laid into each value standing on that node where
    /// it lacks them, shared wherever a part falls whole within what it lacks.
    pub(crate) fn took(&mut self, key: Hash, parts: &[Arc<Buffer>]) {
        for at in self.values.ordered().collect::<Vec<_>>() {
            let value = &mut self.values[at];
            let Kind::Resident(stored) = &value.kind else {
                continue;
            };
            if stored.key != key {
                continue;
            }
            laid(value, parts);
        }
    }
}

/// What each value and all under it cost, and the most any moved a read: a value one other
/// reads sums into its reader's own tree; one more read is summed once into each value over it.
struct Under {
    own: Vec<u128>,
    apart: Vec<Rc<BTreeSet<usize>>>,
    moved: Vec<f64>,
}

impl Under {
    fn of(table: &Table) -> Under {
        let span = table.values.span();
        let mut readers = vec![0u32; span];
        let distinct = |at: usize| {
            let mut reads = table.values[at].reads.clone();
            reads.sort_unstable();
            reads.dedup();
            reads
        };
        for at in table.values.ordered() {
            for read in distinct(at) {
                readers[read] += 1;
            }
        }
        let mut under = Under {
            own: vec![0; span],
            apart: vec![Rc::default(); span],
            moved: vec![0.0; span],
        };
        for at in table.values.ordered() {
            let (mut own, mut moved) = (table.planned[at], table.values[at].moved);
            let mut sets: Vec<Rc<BTreeSet<usize>>> = Vec::new();
            let mut more = Vec::new();
            for read in distinct(at) {
                crate::steps::step(1);
                moved = moved.max(under.moved[read]);
                match readers[read] {
                    1 => own += under.own[read],
                    _ => more.push(read),
                }
                let held = &under.apart[read];
                if !held.is_empty() && !sets.iter().any(|set| Rc::ptr_eq(set, held)) {
                    sets.push(Rc::clone(held));
                }
            }
            under.apart[at] = match (sets.len(), more.is_empty()) {
                (0, true) => Rc::default(),
                (1, true) => Rc::clone(&sets[0]),
                _ => Rc::new(
                    sets.iter()
                        .flat_map(|s| s.iter())
                        .copied()
                        .chain(more)
                        .collect(),
                ),
            };
            (under.own[at], under.moved[at]) = (own, moved);
        }
        under
    }

    fn of_value(&self, at: usize) -> (u128, f64) {
        crate::steps::step(self.apart[at].len());
        let apart = self.apart[at].iter().map(|s| self.own[*s]).sum::<u128>();
        (self.own[at] + apart, self.moved[at])
    }
}

#[cfg(test)]
mod tests {
    use crate::{RenderConfig, Tier, render};

    /// The steps a render folds, naming its nodes and bounding, over a chain `depth` nodes
    /// deep, each reading the one below 10 ms late, every node a miss.
    fn folded(depth: usize) -> u64 {
        folded_as(depth, "@P(t - 10ms)")
    }

    fn folded_as(depth: usize, body: &str) -> u64 {
        let mut files = sva_ast::Composition::new();
        let decays = "sample(crop(sin(2*pi*440*t)*exp(-t/0.1), 0s, 10s))\n";
        files.insert("c0", decays);
        for k in 1..=depth {
            files.insert(
                format!("c{k}"),
                body.replace('P', &format!("c{}", k - 1)) + "\n",
            );
        }
        let g = sva_ast::load(&files).expect("a composition");
        let before = crate::steps::taken();
        let top = format!("c{depth}");
        render(&g, &top, RenderConfig::at(8_000), &Tier::default()).expect("a render");
        crate::steps::taken() - before
    }

    /// Four times the nodes, four times the steps: no node walks what lies under it.
    #[test]
    fn a_chains_nodes_and_bounds_fold_steps_linear_in_its_nodes() {
        let (short, long) = (folded(100), folded(400));
        assert!(short > 0);
        assert!(long <= 4 * short, "{short} then {long}");
    }

    /// Four times the nodes, under five times the steps.
    #[test]
    fn a_chain_of_scaled_and_summed_reads_folds_steps_linear_in_its_nodes() {
        for body in ["0.9*@P(t - 10ms)", "@P(t)*0.5 + @P(t - 10ms)*0.4"] {
            let (short, long) = (folded_as(50, body), folded_as(200, body));
            assert!(long < 5 * short, "{body}: {short} then {long}");
        }
    }
}
