// Concern: what the store answers a value with before it computes, and what it leaves there after | Non-concern: the store's cap and evictions | IO: (value, Recording) -> samples loaded, entries

use std::collections::BTreeMap;
use std::sync::Arc;

use sva_formula::Hash;
use sva_samples::{Buffer, Extent, Machine, MachineState, NodeRenderer, Tape};

use super::segments::Segments;
use super::value::{Held, Kind, Value};
use crate::cache::{Expected, Outcome, Payload, PayloadKind, Recording, Run, frames_key};

/// Where a value sits in the store: its key, and what a policy weighs of it.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    pub(crate) key: Hash,
    /// A run cut where its switches turn, each segment under the value's identity before the
    /// next switch, the last under its own; the first starts where its run does.
    pub(crate) segments: Vec<(i64, Hash)>,
    pub(crate) fork: bool,
    pub(crate) target: bool,
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

    fn keyed(&self, value: &Value) -> Hash {
        match value.kind {
            Kind::Frames { window, hop } => frames_key(self.key, window, hop),
            _ => self.key,
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

/// What the store holds of `value`, laid in beside what it holds itself; `false` where the
/// store answered nothing.
pub(crate) fn load(value: &mut Value, place: &mut Place, recording: &Recording) -> bool {
    place.looked = true;
    if matches!(value.kind, Kind::Stored { .. }) {
        return false;
    }
    let kind = Place::kind(value);
    let (rate, width) = (value.grid.rate, value.width);
    let stamp = recording.stamp(place.slot, place.fork, kind);
    if kind == PayloadKind::Run {
        return resumed(value, place, recording);
    }
    let expected = match kind {
        PayloadKind::Frames => Expected::Frames,
        _ => Expected::Segments { rate, width },
    };
    let Some(entry) = recording.load(place.keyed(value), expected, stamp) else {
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
    let stamp = recording.stamp(place.slot, place.fork, PayloadKind::Run);
    let mut planes = vec![Vec::new(); value.width];
    let (mut base, mut pos, mut reached) = (None, i64::MIN, 0);
    let mut marks: BTreeMap<i64, MachineState> = BTreeMap::new();
    for k in 0..place.segments.len() {
        let Some(run) = recording
            .load(place.segments[k].1, expected, stamp)
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
    if place.noted.is_some() || matches!(value.kind, Kind::Stored { .. }) {
        return;
    }
    let (kind, key) = (Place::kind(value), place.keyed(value));
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
    let (kind, key) = (Place::kind(value), place.keyed(value));
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

/// What a value computed, stored where the policy keeps it: a run segment by segment, each
/// with the states it marked.
pub(crate) fn stored(
    value: &mut Value,
    place: &Place,
    computed: &[Extent],
    recording: &mut Recording,
) {
    let kind = Place::kind(value);
    let stored = matches!(value.kind, Kind::Stored { .. });
    if computed.is_empty() || stored || !value.pure || !recording.stores(place.fork, place.target) {
        return;
    }
    let stamp = recording.stamp(place.slot, place.fork, kind);
    let key = place.keyed(value);
    let label = value.label.clone();
    let payload = match &mut value.held {
        Held::Segments(parts) => Payload::Segments(
            computed
                .iter()
                .flat_map(|e| parts.iter().filter_map(move |b| over(b, *e)))
                .collect(),
        ),
        Held::Frames(Some(frames)) => Payload::Frames(Arc::clone(frames)),
        Held::Frames(None) => return,
        Held::Run(tape) => {
            let (Some(from), Kind::Program(program)) = (computed.first(), &mut value.kind) else {
                return;
            };
            let Some(machine) = &program.machine else {
                return;
            };
            program.marks.insert(tape.end(), machine.state());
            let marks = std::mem::take(&mut program.marks);
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
                recording.store(
                    (*segment, place.noted),
                    Payload::Run(Arc::new(run)),
                    None,
                    stamp,
                );
            }
            return;
        }
    };
    recording.store((key, place.noted), payload, label.as_ref(), stamp);
}

/// `buffer` over `e` where it holds all of it: the part itself where `e` is all it holds.
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

/// What `tape` holds over `e`, where it holds all of it: only those samples are copied.
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
