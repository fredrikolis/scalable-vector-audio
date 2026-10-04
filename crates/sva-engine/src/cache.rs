// Concern: what memory and a disk hold, and the one key they hold it under | Non-concern: what memory keeps and evicts (memory.rs) | IO: (identity, Question) -> key; (Hash) -> a payload

mod codec;
mod index;
pub(crate) mod log;
mod memory;
mod persist;
mod stats;
mod stored;
mod tier;

pub use codec::STORE_FORMAT;
pub use memory::{BYTES_PER_FLOP, Counters, DEFAULT_CACHE_BYTES, DEFAULT_MARK_EVERY};
pub(crate) use memory::{Facts, Known, Memory, Offered};
pub use persist::{Backend, DEFAULT_STORE_BYTES, INDEX_NAME, Persisted, Store};
pub(crate) use stats::Recording;
pub use stats::{CacheStats, Lookup, Outcome};
pub use stored::Stored;
pub use sva_formula::Hash;
pub(crate) use tier::now;
pub use tier::{FETCH_READS, Nothing, Tier};

use std::collections::BTreeMap;
use std::sync::Arc;

use sva_samples::{Buffer, Frames, Grid, Label, MachineState, Profile};

/// A value's segments, a stateful value's run, or one analysis of a value, each shared: a clone
/// hands out the same samples, never a copy of them.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Segments(Vec<Arc<Buffer>>),
    Frames(Arc<Frames>),
    Run(Arc<Run>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    Segments,
    Frames,
    Run,
}

/// One segment of a run: samples, marked states, and the segment before it.
#[derive(Clone)]
pub struct Run {
    pub samples: Arc<Buffer>,
    pub marks: BTreeMap<i64, MachineState>,
    pub parent: Option<Hash>,
}

impl Run {
    pub fn end(&self) -> i64 {
        self.samples.extent().end
    }

    pub fn bytes(&self) -> usize {
        let marks: usize = self.marks.values().map(MachineState::bytes).sum();
        self.samples.len() * self.samples.width * size_of::<f64>() + marks
    }
}

/// The state is the node's own where its samples end, so the samples decide.
impl PartialEq for Run {
    fn eq(&self, other: &Run) -> bool {
        self.samples == other.samples
    }
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run")
            .field("samples", &self.samples.extent())
            .finish()
    }
}

/// An entry not matching this is a miss, never a coercion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    Segments { rate: u32, width: usize },
    Frames,
    Run { rate: u32, width: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub payload: Payload,
    /// FORMAT 9.3: the label is part of the value, so a hit answers with the cold run's.
    pub label: Option<Label>,
}

impl Payload {
    pub fn run(self) -> Option<Arc<Run>> {
        match self {
            Payload::Run(run) => Some(run),
            _ => None,
        }
    }

    pub fn bytes(&self) -> usize {
        match self {
            Payload::Segments(parts) => parts
                .iter()
                .map(|b| b.len() * b.width * size_of::<f64>())
                .sum(),
            Payload::Frames(f) => f.width * f.frames * f.bins * 2 * size_of::<f64>(),
            Payload::Run(run) => run.bytes(),
        }
    }

    pub fn answers(&self, expected: Expected) -> bool {
        match (self, expected) {
            (Payload::Segments(parts), Expected::Segments { rate, width }) => {
                parts.iter().all(|b| b.rate == rate && b.width == width)
            }
            (Payload::Frames(_), Expected::Frames) => true,
            (Payload::Run(run), Expected::Run { rate, width }) => {
                run.samples.rate == rate && run.samples.width == width
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Question<'p> {
    pub(crate) grid: Grid,
    pub(crate) width: usize,
    pub(crate) profile: &'p Profile,
    pub(crate) shape: Shape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Samples,
    Frames { window: usize, hop: usize },
}

/// What a value computes and what is asked of it, never where a reader places it.
pub(crate) fn key(identity: Hash, question: &Question) -> Hash {
    let Question {
        grid,
        width,
        profile,
        shape,
    } = *question;
    let asked = [
        grid.a as u64,
        grid.d as u64,
        u64::from(grid.rate),
        width as u64,
    ];
    let shape = match shape {
        Shape::Samples => [0, 0, 0],
        Shape::Frames { window, hop } => [1, window as u64, hop as u64],
    };
    let tag = [0x6b_65_79_00_00_00_00_01];
    mixed(
        identity,
        &[&asked[..], &profile.deciding(), &shape, &tag].concat(),
    )
}

const ADDRESS_ROTATE: u32 = 17;

pub(crate) fn mixed(seed: Hash, parts: &[u64]) -> Hash {
    let mut lanes = sva_formula::Lanes::<ADDRESS_ROTATE>::from(seed);
    for part in parts {
        lanes.word(*part);
    }
    lanes.finish()
}

/// `more` laid among `parts`, each touching pair joined into one; a shared part is copied only
/// to grow.
pub(crate) fn joined(parts: &mut Vec<Arc<Buffer>>, more: Vec<Arc<Buffer>>) {
    for part in more {
        parts.push(part);
    }
    parts.sort_by_key(|b| b.start);
    let mut out: Vec<Arc<Buffer>> = Vec::with_capacity(parts.len());
    for part in parts.drain(..) {
        match out.last_mut() {
            Some(last) if last.extent().end >= part.start => {
                let from = (last.extent().end - part.start) as usize;
                if from < part.len() {
                    let last = Arc::make_mut(last);
                    for (held, more) in last.planes.iter_mut().zip(&part.planes) {
                        held.extend_from_slice(&more[from.min(more.len())..]);
                    }
                }
            }
            _ => out.push(part),
        }
    }
    *parts = out;
}
