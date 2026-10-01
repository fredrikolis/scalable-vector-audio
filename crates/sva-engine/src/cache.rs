// Concern: declares what a store holds under a content hash and how a value's key is built | Non-concern: what the store keeps and evicts (store.rs) | IO: (Hash) -> a payload

mod codec;
mod index;
pub(crate) mod log;
mod persist;
mod stats;
mod store;
mod stored;

pub use codec::STORE_FORMAT;
pub use persist::{
    Backend, DEFAULT_STORE_BYTES, INDEX_NAME, NoStore, Persisted, Store, Stored, Through,
};
pub(crate) use stats::Recording;
pub use stats::{CacheStats, Lookup, Outcome};
pub(crate) use store::joined;
pub use store::{Cache, CachePolicy, DEFAULT_CACHE_BYTES, DEFAULT_MARK_EVERY, PrunePolicy};
pub(crate) use stored::node_key;
pub use sva_formula::Hash;

use std::collections::BTreeMap;
use std::sync::Arc;

use sva_samples::{Buffer, Frames, Label, MachineState};

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

pub fn frames_key(value: Hash, window: usize, hop: usize) -> Hash {
    mixed(
        value,
        &[window as u64, hop as u64, 0x66_72_61_6d_65_73_00_01],
    )
}

/// One value at one rate, width and profile, never where a reader places it.
pub fn value_key(
    identity: Hash,
    step: (i128, i128),
    rate: u32,
    width: usize,
    profile: &sva_samples::Profile,
) -> Hash {
    mixed(
        identity,
        &[
            step.0 as u64,
            step.1 as u64,
            u64::from(rate),
            width as u64,
            profile.precision_bits as u64,
            profile.ceiling_hz.to_bits(),
            profile.prune_db.to_bits(),
            0x76_61_6c_75_65_00_00_01,
        ],
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
