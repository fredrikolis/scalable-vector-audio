// Concern: builds a driven node, its clock, reach and store run, and runs it over a block | Non-concern: the order nodes run in, what an edit keeps | IO: (NodeId) -> Driven; (from, to) -> its tape

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Hash, Held, NodeId};
use sva_samples::{Buffer, Extent, Machine, MachineState, NodeRenderer, Rows, Slot, Tape, Window};

use super::super::pointwise::{self, Point};
use super::super::{Lenses, Render, collapse_refused, sampled};
use crate::cache::{Expected, Outcome, Payload, Recording, Run, run_key};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::refs;
use crate::typing::Value;

pub(in crate::render) enum Kind {
    /// Held whole before any reader ran.
    Whole,
    Rows(Rows),
    Point(Box<Point>),
    Machine {
        machine: Machine,
        /// Each slot's node, as an index into the driver's nodes.
        reads: Vec<usize>,
    },
    Ended,
}

pub(in crate::render) struct Driven {
    pub(in crate::render) id: NodeId,
    pub(in crate::render) kind: Kind,
    /// A sample's price where no row prices it.
    per_sample: u128,
    pub(in crate::render) width: usize,
    /// How many samples before a block's start any reader, itself included, reaches.
    pub(in crate::render) keep: usize,
    /// How far behind the root's its own clock runs.
    pub(in crate::render) lag: i64,
    /// Each node it reads, as an index into the driver's nodes, and the shift.
    pub(in crate::render) reads: Vec<(usize, i64)>,
    pub(super) own: usize,
    trailing: bool,
    pub(in crate::render) extent: Extent,
    pub(in crate::render) support: Extent,
    pub(in crate::render) tape: Tape,
    pub(in crate::render) run: Option<Recorded>,
}

impl Recorded {
    /// Whether `other`'s samples before `at` are this run's: the two agree there.
    pub(super) fn shares(&self, other: &Recorded, at: i64) -> bool {
        self.segments.before(at) == other.segments.before(at)
    }

    /// Takes all a run of the same node recorded.
    pub(super) fn continues(&mut self, was: Recorded) {
        (self.stored, self.held, self.marks) = (was.stored, was.held, was.marks);
    }
}

pub(in crate::render) struct Recorded {
    segments: Segments,
    /// How far the store holds this run, and how far it holds each segment's entry.
    pub(super) stored: i64,
    held: Vec<i64>,
    /// Every sample from where its state starts is kept.
    pub(super) records: bool,
    pub(super) loaded: bool,
    /// The states passed since the store last took them, and how far apart a ladder keeps them.
    pub(super) marks: BTreeMap<i64, MachineState>,
    every: usize,
}

/// A run cut where its switches turn: each segment keyed by the node's identity before the
/// next switch, the last by its whole identity.
pub(in crate::render) struct Segments {
    starts: Vec<i64>,
    keys: Vec<Hash>,
}

impl Segments {
    fn of(shell: &Render, id: NodeId, extent: Extent) -> Result<Segments, EngineError> {
        let width = usize::from(shell.tys.ty(id).width).max(1);
        let rate = shell.rate();
        let points = shell.prefixes(|walk| walk.change_points(id));
        let (mut starts, mut keys) = (vec![extent.start], Vec::new());
        for &at in points.iter().filter(|at| **at > extent.start) {
            let before = shell.prefixes(|walk| walk.prefix_identity(id, at))?;
            let key = shell.keyed(run_key(before, rate, width));
            if keys.last() != Some(&key) {
                keys.push(key);
                starts.push(at);
            }
        }
        let whole = run_key_of(shell, id)?;
        match keys.last() == Some(&whole) {
            true => {
                starts.pop();
            }
            false => keys.push(whole),
        }
        Ok(Segments { starts, keys })
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    /// The node's identity before `at`: the segment holding the sample before it.
    fn before(&self, at: i64) -> Hash {
        let k = self.starts[1..].iter().take_while(|s| **s < at).count();
        self.keys[k]
    }

    fn end(&self, k: usize) -> i64 {
        self.starts.get(k + 1).copied().unwrap_or(i64::MAX)
    }

    fn whole(&self) -> Hash {
        *self.keys.last().expect("a run has a segment")
    }

    fn parent(&self, k: usize) -> Option<Hash> {
        k.checked_sub(1).map(|p| self.keys[p])
    }

    /// Where a machine with call sites keeps its state: each switch, and a ladder.
    fn marks_in(&self, from: i64, to: i64, every: usize) -> Vec<i64> {
        let base = self.starts[0];
        let step = every as i64;
        let mut out: Vec<i64> = self.starts[1..]
            .iter()
            .copied()
            .filter(|at| from < *at && *at <= to)
            .collect();
        let mut at = base + ((from - base).div_euclid(step) + 1) * step;
        while at <= to {
            out.push(at);
            at += step;
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// How a driver holds its nodes' samples.
pub(in crate::render) enum Hold {
    /// A block, and as far back as its readers reach; the root `root_keep` samples besides.
    Trailing { block: usize, root_keep: usize },
    /// Every sample, the nodes held here whole before any reader runs.
    Every(BTreeMap<NodeId, Buffer>),
}

/// Each of `order`: a run the store holds is read, except where `unloaded` names an edit's.
pub(in crate::render) fn built(
    shell: &Render,
    order: &[NodeId],
    mut hold: Hold,
    lenses: &Lenses,
    unloaded: &dyn Fn(usize) -> bool,
) -> Result<Vec<Driven>, EngineError> {
    let trailing = matches!(hold, Hold::Trailing { .. });
    let mut holds: Vec<usize> = Vec::new();
    let mut nodes: Vec<Driven> = Vec::new();
    let mut index: BTreeMap<NodeId, usize> = BTreeMap::new();
    for (at, &id) in order.iter().enumerate() {
        let extent = shell.extents.of(id);
        let support = shell.extents.support(id);
        index.insert(id, nodes.len());
        if let Hold::Every(held) = &mut hold
            && let Some(buffer) = held.remove(&id)
        {
            nodes.push(whole(id, buffer, extent, support));
            continue;
        }
        let mut run = match lenses.runs.contains(&id) && !extent.is_empty() {
            true => found(shell, id, extent, lenses, !unloaded(at))?,
            false => None,
        };
        let whole_run =
            |(_, loaded): &(Recorded, Loaded)| loaded.samples.extent().end >= extent.end;
        if !trailing && run.as_ref().is_some_and(whole_run) {
            let (mut recorded, loaded) = run.take().expect("a whole run");
            let mut node = whole(id, loaded.samples.over(extent, support), extent, support);
            recorded.records = false;
            node.run = Some(recorded);
            nodes.push(node);
            continue;
        }
        let Built {
            kind,
            width,
            reads,
            own,
            held,
        } = kind(shell, id, &nodes, &index)?;
        holds.extend(held);
        let per_sample = match kind {
            Kind::Whole | Kind::Rows(_) | Kind::Ended => 0,
            Kind::Point(_) | Kind::Machine { .. } => crate::flops::per_sample(shell, id),
        };
        let mut node = Driven {
            id,
            kind,
            per_sample,
            width,
            keep: 0,
            lag: 0,
            reads,
            own,
            trailing,
            extent,
            support,
            tape: Tape::new(width, 0, extent.start),
            run: None,
        };
        if let Some((recorded, run)) = run {
            node.run = Some(recorded);
            node.preload(run);
        }
        nodes.push(node);
    }
    let root = index.get(&shell.root).copied();
    clocked(&mut nodes, root, trailing);
    for at in holds {
        nodes[at].keep = WHOLE;
    }
    if let Hold::Trailing { block, root_keep } = hold {
        let last = shell.range.map_or(i64::MAX, |range| range.end);
        for (at, node) in nodes.iter_mut().enumerate() {
            if Some(at) == root {
                node.keep = node.keep.max(root_keep);
            }
            let leaf = node.reads.is_empty()
                && matches!(&node.kind, Kind::Machine { machine, .. } if machine.stateful());
            let ends = node.extent.end.saturating_add(node.lag) < last;
            if let Some(run) = &mut node.run {
                run.records &= leaf || ends;
            }
            if node.tape.end() == node.extent.start {
                let keep = match node.extent.is_bounded() {
                    true => node.keep.min(node.extent.len()),
                    false => node.keep.min(block),
                };
                node.tape = Tape::new(node.width, keep + block, node.extent.start);
            }
        }
    }
    Ok(nodes)
}

/// A keep past any extent: a reader whose index scales may come back to any sample.
const WHOLE: usize = usize::MAX;

/// Each node's lag, the least its readers need, and its keep, the most they reach back.
fn clocked(nodes: &mut [Driven], root: Option<usize>, trailing: bool) {
    let mut lags: Vec<Option<i64>> = vec![None; nodes.len()];
    if let Some(root) = root {
        lags[root] = Some(0);
    }
    for at in (0..nodes.len()).rev() {
        if trailing && (lags[at].is_none() || nodes[at].extent.is_empty()) {
            nodes[at].kind = Kind::Ended;
            nodes[at].reads.clear();
        }
        let lag = lags[at].unwrap_or(0);
        nodes[at].lag = lag;
        nodes[at].keep = nodes[at].own;
        for &(read, shift) in &nodes[at].reads {
            let wants = lag - shift;
            lags[read] = Some(lags[read].map_or(wants, |held| held.min(wants)));
        }
    }
    for at in 0..nodes.len() {
        for (read, shift) in nodes[at].reads.clone() {
            let back = (nodes[at].lag - shift - nodes[read].lag).max(0) as usize;
            nodes[read].keep = nodes[read].keep.max(back);
        }
    }
}

/// A machine a stream and a whole render compute alike, so one run answers both: it reads
/// only such machines, and holds call-site state only where it reads nothing.
pub(in crate::render) fn runnable(shell: &Render, order: &[NodeId]) -> BTreeSet<NodeId> {
    let mut out = BTreeSet::new();
    for &id in order {
        if !matches!(shell.tys.ty(id).held, Held::Sampled)
            || matches!(shell.tys.value(id), Value::Cast(Cast::Istft, _))
        {
            continue;
        }
        let Ok(program) = sampled::program_reading(shell, id, &|_| 1) else {
            continue;
        };
        let mut ahead = false;
        crate::render::extent::leaves(&program.renderer, &mut |leaf| {
            if let NodeRenderer::Read { map, .. } = leaf {
                ahead |= map.ahead();
            }
        });
        let reads = program.reads.iter().all(|r| out.contains(r));
        if !ahead && reads && (program.layout.sites.is_empty() || program.reads.is_empty()) {
            out.insert(id);
        }
    }
    out
}

/// What the store answers a node with: its samples from where its state starts, and the
/// state where they end where it holds call sites.
pub(in crate::render) struct Loaded {
    samples: Buffer,
    state: Option<MachineState>,
}

/// Segment by segment from the first: a segment's samples up to the next switch, on to the
/// next segment where it marked its state there, else from its last mark. A run starting after
/// the extent is no prefix of it; a stateless one starting before is.
fn found(
    shell: &Render,
    id: NodeId,
    extent: Extent,
    lenses: &Lenses,
    looks: bool,
) -> Result<Option<(Recorded, Loaded)>, EngineError> {
    let (Some(lens), Some(recording)) = (lenses.at(id), lenses.recording) else {
        return Ok(None);
    };
    let (rate, width) = (shell.rate(), usize::from(shell.tys.ty(id).width).max(1));
    let segments = Segments::of(shell, id, extent)?;
    let mut recorded = Recorded {
        held: vec![i64::MIN; segments.len()],
        segments,
        stored: extent.start,
        records: lens.keeps(),
        loaded: false,
        marks: BTreeMap::new(),
        every: recording.mark_every(),
    };
    let mut loaded = Loaded {
        samples: Buffer::silence(rate, width, 0),
        state: None,
    };
    loaded.samples.start = extent.start;
    if !looks {
        return Ok(Some((recorded, loaded)));
    }
    let sites = !sampled::program_reading(shell, id, &|_| 1)?
        .layout
        .sites
        .is_empty();
    let expected = Expected::Run { rate, width };
    let segments = &recorded.segments;
    let (mut planes, mut pos) = (vec![Vec::new(); width], extent.start);
    let (mut state, mut reached) = (None, None);
    for k in 0..segments.len() {
        let (start, boundary) = (segments.starts[k], segments.end(k));
        let Some(run) = lens
            .peek(segments.keys[k], expected)
            .and_then(|entry| entry.payload.run())
        else {
            break;
        };
        let placed = match k {
            0 => run.samples.start <= start && start < run.end(),
            _ => run.samples.start == start && run.parent == segments.parent(k),
        };
        if !placed {
            break;
        }
        recorded.held[k] = run.end();
        if k == 0 {
            recorded.records &= run.samples.start == extent.start;
        }
        let upto = boundary.min(run.end()).min(extent.end);
        for (c, plane) in planes.iter_mut().enumerate() {
            let at = |n: i64| run.samples.plane(c)[(n - run.samples.start) as usize];
            plane.extend((pos..upto).map(at));
        }
        (pos, reached) = (upto, Some(k));
        let taken = run
            .marks
            .range(start..=upto)
            .map(|(at, m)| (*at, m.clone()));
        recorded.marks.extend(taken);
        if upto >= extent.end || (upto == boundary && !sites) {
            state = run.marks.get(&upto).cloned();
            if upto >= extent.end {
                break;
            }
            continue;
        }
        if upto == boundary && run.marks.contains_key(&boundary) {
            state = run.marks.get(&boundary).cloned();
            continue;
        }
        if sites {
            match run.marks.range(start..=upto).next_back() {
                Some((at, mark)) => (pos, state) = (*at, Some(mark.clone())),
                None => pos = start,
            }
            planes
                .iter_mut()
                .for_each(|p| p.truncate((pos - extent.start) as usize));
            recorded.marks.retain(|at, _| *at <= pos);
        }
        break;
    }
    if let Some(k) = reached {
        for key in segments.keys[..=k].iter().rev() {
            lens.touch(*key);
        }
    }
    let outcome = match reached {
        _ if pos == extent.start => Outcome::ComputedNotStored,
        Some(k) if k + 1 == segments.len() => Outcome::Hit,
        _ => Outcome::Prefix,
    };
    lens.note(shell.tys.name(id), segments.whole(), outcome);
    if pos > extent.start {
        recorded.stored = pos;
        recorded.loaded = true;
        loaded.samples = Buffer::of_planes(rate, planes);
        loaded.samples.start = extent.start;
        loaded.state = state;
    }
    Ok(Some((recorded, loaded)))
}

/// Whether the store's segments of `id` hold every sample of its extent, one after another.
pub(in crate::render) fn covered(shell: &Render, id: NodeId, recording: &Recording) -> bool {
    let extent = shell.extents.of(id);
    let Ok(segments) = Segments::of(shell, id, extent) else {
        return false;
    };
    for k in 0..segments.len() {
        let Some((span, parent)) = recording.run_span(segments.keys[k]) else {
            return false;
        };
        let start = segments.starts[k];
        let placed = match k {
            0 => span.start <= start && start < span.end,
            _ => span.start == start && parent == segments.parent(k),
        };
        let upto = segments.end(k).min(span.end).min(extent.end);
        if !placed || upto >= extent.end || upto < segments.end(k) {
            return placed && upto >= extent.end;
        }
    }
    false
}

pub(in crate::render) fn run_key_of(shell: &Render, id: NodeId) -> Result<Hash, EngineError> {
    let width = usize::from(shell.tys.ty(id).width).max(1);
    Ok(shell.keyed(run_key(shell.identity(id)?, shell.rate(), width)))
}

/// A node held whole before any reader runs.
pub(in crate::render) fn whole(
    id: NodeId,
    buffer: Buffer,
    extent: Extent,
    support: Extent,
) -> Driven {
    Driven {
        id,
        kind: Kind::Whole,
        per_sample: 0,
        width: buffer.planes.len().max(1),
        keep: 0,
        lag: 0,
        reads: Vec::new(),
        own: 0,
        trailing: false,
        extent,
        support,
        tape: Tape::from(buffer),
        run: None,
    }
}

struct Built {
    kind: Kind,
    width: usize,
    reads: Vec<(usize, i64)>,
    pub(super) own: usize,
    /// The nodes it reads at scales or times that move, which it may read anywhere back.
    held: Vec<usize>,
}

fn kind(
    shell: &Render,
    id: NodeId,
    nodes: &[Driven],
    index: &BTreeMap<NodeId, usize>,
) -> Result<Built, EngineError> {
    match shell.tys.ty(id).held {
        Held::Frames => Err(no_stream(shell, id, "a short-time spectrum")),
        Held::Sampled => machine(shell, id, nodes, index),
        _ => closed_form(shell, id).map(|(kind, width)| Built {
            kind,
            width,
            reads: Vec::new(),
            own: 0,
            held: Vec::new(),
        }),
    }
}

fn machine(
    shell: &Render,
    id: NodeId,
    nodes: &[Driven],
    index: &BTreeMap<NodeId, usize>,
) -> Result<Built, EngineError> {
    if let Value::Cast(Cast::Istft, _) = shell.tys.value(id) {
        return Err(no_stream(shell, id, "an inverse short-time transform"));
    }
    let width_of = |r: NodeId| index.get(&r).map_or(1, |at| nodes[*at].width);
    let extent = shell.extents.of(id);
    let program = sampled::program_reading(shell, id, &width_of)?;
    let slots: Vec<usize> = program
        .reads
        .iter()
        .map(|r| index.get(r).copied().ok_or_else(|| unheld(shell, *r)))
        .collect::<Result<_, _>>()?;
    let (mut reads, mut own, mut ahead, mut held) = (Vec::new(), 0, false, Vec::new());
    crate::render::extent::leaves(&program.renderer, &mut |leaf| match leaf {
        NodeRenderer::Read {
            slot: Slot::Read(id),
            map,
        } if map.a == map.d => {
            ahead |= map.ahead();
            let lead = map.lead();
            reads.push((slots[id.0 as usize], lead));
            if map.least() != lead {
                reads.push((slots[id.0 as usize], map.least()));
            }
        }
        NodeRenderer::Read {
            slot: Slot::Read(id),
            ..
        } => {
            reads.push((slots[id.0 as usize], 0));
            held.push(slots[id.0 as usize]);
        }
        NodeRenderer::Read {
            slot: Slot::Own,
            map,
        } if map.a == map.d => {
            own = own.max((-map.least()).max(0) as usize);
        }
        NodeRenderer::Read {
            slot: Slot::Own, ..
        } => own = WHOLE,
        _ => {}
    });
    if ahead {
        return Err(no_stream(
            shell,
            id,
            "a read ahead of the sample it is taken at",
        ));
    }
    let live: Vec<Extent> = slots
        .iter()
        .map(|&r| match nodes[r].kind {
            Kind::Whole => {
                let tape = &nodes[r].tape;
                sampled::live(tape.planes(), tape.base(), nodes[r].support)
            }
            _ => nodes[r].support,
        })
        .collect();
    let rate = shell.rate();
    let span = (extent.start, extent.end);
    let machine = Machine::live(&program.renderer, &program.layout, rate, span, &live)
        .map_err(|e| sampled::refused(shell, id, &e))?;
    Ok(Built {
        width: machine.width(),
        kind: Kind::Machine {
            machine,
            reads: slots,
        },
        reads,
        own,
        held,
    })
}

/// `collapse_closed_form`'s choice between the rows and the point sampler.
fn closed_form(shell: &Render, id: NodeId) -> Result<(Kind, usize), EngineError> {
    let written = match refs::resolve(&shell.tys, id, shell.tys.ty(id).held) {
        Ok(refs::Read::Substitute(form)) => Some(*form),
        _ => None,
    };
    let sum = refs::spectral_sum_of(&shell.tys, id, shell.tys.var(id));
    let (rate, profile) = (shell.rate(), &shell.config.profile);
    let rows = match (&sum, &written) {
        (Err(_), None) => return point(shell, id),
        (Ok(sum), written) => Rows::of_spectral_sum_or_point(sum, written.as_ref(), rate, profile),
        (Err(_), Some(form)) => Rows::of(form, rate, profile),
    }
    .map_err(|e| match e {
        sva_samples::CollapseError::NoBlockRow => no_stream(shell, id, "a closed form in f"),
        e => collapse_refused(shell, id, &e),
    })?;
    let width = rows.width();
    Ok((Kind::Rows(rows), width))
}

fn point(shell: &Render, id: NodeId) -> Result<(Kind, usize), EngineError> {
    let tree = pointwise::plan(shell, id)?;
    if pointwise::reads_samples(&tree) {
        return Err(no_stream(
            shell,
            id,
            "a point-sampled form reading another node's samples",
        ));
    }
    let width = usize::from(shell.tys.ty(id).width).max(1);
    Ok((Kind::Point(Box::new(tree)), width))
}

impl Driven {
    fn preload(&mut self, loaded: Loaded) {
        let end = loaded.samples.extent().end.min(self.extent.end);
        let carried = match (&mut self.kind, &loaded.state) {
            (Kind::Machine { machine, .. }, _) if !machine.stateful() || end >= self.extent.end => {
                true
            }
            (Kind::Machine { machine, .. }, Some(last)) => machine.carry(last),
            (Kind::Machine { .. }, None) => false,
            _ => true,
        };
        if loaded.samples.is_empty() || !carried {
            if let Some(recorded) = &mut self.run {
                (recorded.loaded, recorded.stored) = (false, self.extent.start);
                recorded.marks.clear();
            }
            return;
        }
        let mut tape = Tape::from(loaded.samples);
        tape.forget_before(self.extent.start);
        tape.cut(end);
        self.tape = tape;
    }

    /// Samples up to `to` on its own clock, every node it reads already there; the span it
    /// computed.
    pub(in crate::render) fn run(
        &mut self,
        shell: &Render,
        done: &[Driven],
        from: i64,
        to: i64,
    ) -> Result<(i64, i64), EngineError> {
        if self.forgets() {
            self.tape.forget_before(from.saturating_sub(self.back()));
        }
        let to = to.min(self.extent.end);
        let start = self.tape.end();
        if self.extent.is_empty() || to <= start {
            return Ok((start, start));
        }
        let id = self.id;
        match &mut self.kind {
            Kind::Whole | Kind::Ended => {}
            Kind::Rows(rows) => {
                let from = self.tape.end();
                rows.extend(to, &mut self.tape)
                    .map_err(|e| collapse_refused(shell, id, &e))?;
                let grown = (self.tape.end() - from) as usize;
                let planes = self.tape.planes().iter().map(|p| &p[p.len() - grown..]);
                super::super::finite(shell, id, planes.flatten())?;
            }
            Kind::Point(tree) => {
                let step = 1.0 / f64::from(shell.rate());
                for n in self.tape.end()..to {
                    for c in 0..self.width {
                        let at = pointwise::Instant {
                            t: n as f64 * step,
                            n: Some(n),
                        };
                        let v = pointwise::value(shell, tree, c, at)
                            .map_err(|e| pointwise::refused(shell, id, &e))?;
                        self.tape.push(c, v.re);
                    }
                }
            }
            Kind::Machine { machine, reads } => {
                let windows: Vec<Window> = reads
                    .iter()
                    .map(|&r| done[r].tape.within(done[r].support))
                    .collect();
                let refused = |e| sampled::refused(shell, id, &e);
                let marking = self
                    .run
                    .as_mut()
                    .filter(|r| r.records && machine.stateful());
                let marks = marking.as_ref().map_or(Vec::new(), |run| {
                    run.segments.marks_in(self.tape.end(), to, run.every)
                });
                let first = marks.first().map_or(to, |at| *at);
                machine
                    .run_to(first, &windows, &mut self.tape)
                    .map_err(refused)?;
                if let Some(run) = marking {
                    for (k, &at) in marks.iter().enumerate() {
                        if k > 0 {
                            machine
                                .run_on(at, &windows, &mut self.tape)
                                .map_err(refused)?;
                        }
                        run.marks.insert(at, machine.state());
                    }
                }
                machine
                    .run_on(to, &windows, &mut self.tape)
                    .map_err(refused)?;
            }
        }
        Ok((start, self.tape.end()))
    }

    pub(super) fn forgets(&self) -> bool {
        self.trailing && self.run.as_ref().is_none_or(|run| !run.records)
    }

    /// `(priced flops, waves)` over `[from, to)` of its extent: a pointwise tree's waves go
    /// uncounted.
    pub(in crate::render) fn work(&self, from: i64, to: i64) -> (u128, Option<u128>) {
        let (from, to) = (from.max(self.extent.start), to.min(self.extent.end));
        let n = (to - from).max(0) as u128;
        match &self.kind {
            Kind::Rows(rows) => {
                let (priced, waves) = rows.work(from, to.max(from));
                (priced, Some(waves))
            }
            Kind::Point(_) => (self.per_sample * n, None),
            Kind::Whole | Kind::Machine { .. } | Kind::Ended => (self.per_sample * n, Some(0)),
        }
    }

    /// `keep` in samples, where `WHOLE` reaches back past any instant.
    fn back(&self) -> i64 {
        i64::try_from(self.keep).unwrap_or(i64::MAX)
    }

    pub(in crate::render) fn spent(&self, local: i64) -> bool {
        !matches!(self.kind, Kind::Ended)
            && !self.extent.is_empty()
            && local.saturating_sub(self.back()) >= self.extent.end
    }

    pub(in crate::render) fn end(&mut self, shell: &Render, lenses: &Lenses) {
        self.store(shell, lenses);
        self.kind = Kind::Ended;
        self.tape = Tape::new(self.width, 0, self.extent.end);
        self.run = None;
    }

    /// What it recorded past what the store holds, segment by segment: a segment longer
    /// than its entry replaces it, and one no longer adds only its marks.
    pub(in crate::render) fn store(&mut self, shell: &Render, lenses: &Lenses) {
        let Some(recorded) = &mut self.run else {
            return;
        };
        let end = self.tape.end();
        if !recorded.records || end <= recorded.stored || self.tape.base() != self.extent.start {
            return;
        }
        let Some(lens) = lenses.at(self.id) else {
            return;
        };
        if let Kind::Machine { machine, .. } = &self.kind
            && machine.stateful()
        {
            recorded.marks.insert(end, machine.state());
        }
        let samples = self.tape.clone().into_buffer(shell.rate());
        let segments = &recorded.segments;
        for k in 0..segments.len() {
            let (from, to) = (segments.starts[k], segments.end(k).min(end));
            if to <= from {
                break;
            }
            let marks: BTreeMap<i64, MachineState> = recorded
                .marks
                .range(from..=to)
                .map(|(at, m)| (*at, m.clone()))
                .collect();
            if to <= recorded.held[k] {
                lens.mark(segments.keys[k], marks);
                continue;
            }
            let run = Run {
                samples: samples.over(Extent::new(from, to), samples.extent()),
                marks,
                parent: segments.parent(k),
            };
            lens.store(segments.keys[k], &Payload::Run(Box::new(run)), None);
            recorded.held[k] = to;
        }
        recorded.stored = end;
    }

    /// Held only where its tape ends at `at`; a machine with no call site holds none.
    pub(in crate::render) fn state_at(&self, at: i64) -> Option<MachineState> {
        let Kind::Machine { machine, .. } = &self.kind else {
            return None;
        };
        let end = self.tape.end();
        (end == at || (!machine.stateful() && end >= at)).then(|| machine.state())
    }

    pub(in crate::render) fn bytes(&self) -> usize {
        let tape = self.tape.capacity() * self.tape.width() * size_of::<f64>();
        let state = match &self.kind {
            Kind::Machine { machine, .. } => machine.bytes(),
            _ => 0,
        };
        tape + state
    }
}

pub(in crate::render) fn no_stream(shell: &Render, id: NodeId, class: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.no_stream".to_string(),
        message: format!(
            "`{}` is {class}, which no block of a stream reads on its own",
            shell.tys.name(id)
        ),
        location: Located::at(shell.tys.name(id), None),
        help: "render it to a stated end instead of streaming it".to_string(),
    })
}

fn unheld(shell: &Render, id: NodeId) -> EngineError {
    EngineError::UnknownNode(shell.tys.name(id).to_string())
}

#[cfg(test)]
mod tests {
    use super::Segments;
    use crate::cache::{Cache, Outcome, PayloadKind};
    use crate::render::{RenderConfig, plan, render};

    fn config() -> RenderConfig {
        RenderConfig::seconds(8_000, 0.5)
    }

    const STRING: &str = "release = inf\nchaigne_askenfelt(f0, damper_r=0.1*crop(min(1, \
        (t - release)/0.03), release, inf))\n";

    fn rendered(g: &sva_ast::Graph, cache: Option<&Cache>) -> (Vec<f64>, Vec<Outcome>) {
        let held = render(g, "released", config(), cache).expect("a render");
        let runs = held.cache_stats.iter().flat_map(|s| s.lookups.clone());
        let found = runs
            .filter(|l| l.kind == PayloadKind::Run)
            .map(|l| l.outcome);
        let samples = held.output(held.root).expect("a buffer").plane(0).to_vec();
        (samples, found.collect())
    }

    /// A segment is read only on from its parent: with the held run gone, a stored release
    /// is a miss, computed from its start.
    #[test]
    fn a_segment_whose_parent_is_gone_is_a_miss() {
        let mut files = sva_ast::Composition::new();
        files
            .insert("string", STRING)
            .insert("held", "@string(t, f0=261.63)\n")
            .insert("released", "@string(t, f0=261.63, release=0.3)\n");
        let g = sva_ast::load(&files).expect("a composition");
        let cache = Cache::new();
        render(&g, "held", config(), Some(&cache)).expect("held");
        let (cold, _) = rendered(&g, None);
        assert_eq!(rendered(&g, Some(&cache)).0, cold);
        let planned = plan(&g, "released", config()).expect("a plan");
        for &id in &planned.schedule.materialize {
            let segments = Segments::of(&planned, id, planned.extents.of(id)).expect("keys");
            if segments.len() > 1 {
                cache.forget(segments.keys[0]);
            }
        }
        let (again, found) = rendered(&g, Some(&cache));
        assert_eq!(again, cold);
        assert!(
            found.iter().all(|o| *o == Outcome::ComputedStored),
            "{found:?}"
        );
    }
}
