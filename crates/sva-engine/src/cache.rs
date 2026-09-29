// Concern: declares what a store holds under a content hash and how a key is built | Non-concern: what the store keeps and evicts (store.rs) | IO: (Hash) -> a payload

mod stats;
mod store;

pub use stats::{CacheStats, Lookup, Outcome};
pub(crate) use stats::{Lens, Recording};
pub use store::{Cache, CachePolicy, DEFAULT_CACHE_BYTES, DEFAULT_MARK_EVERY, PrunePolicy};
pub use sva_formula::Hash;

use std::collections::BTreeMap;

use sva_formula::SpectralSum;
use sva_samples::{Buffer, Frames, Label, MachineState};

/// A spectral sum, one collapse of it, one analysis of that collapse, or a machine's run.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Samples(Box<Buffer>),
    Frames(Box<Frames>),
    Symbolic(Box<SpectralSum>),
    Run(Box<Run>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    Samples,
    Frames,
    Symbolic,
    Run,
}

/// One segment of a machine node's run: its samples, the state at each marked index, and the
/// segment before it.
#[derive(Clone)]
pub struct Run {
    pub samples: Buffer,
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
    Samples {
        rate: u32,
        width: usize,
        samples: usize,
    },
    Frames,
    Symbolic,
    Run {
        rate: u32,
        width: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub payload: Payload,
    /// FORMAT 9.3: the label is part of the value, so a hit answers with the cold run's.
    pub label: Option<Label>,
}

impl Expected {
    pub fn kind(self) -> PayloadKind {
        match self {
            Expected::Samples { .. } => PayloadKind::Samples,
            Expected::Frames => PayloadKind::Frames,
            Expected::Symbolic => PayloadKind::Symbolic,
            Expected::Run { .. } => PayloadKind::Run,
        }
    }
}

impl Payload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            Payload::Samples(_) => PayloadKind::Samples,
            Payload::Frames(_) => PayloadKind::Frames,
            Payload::Symbolic(_) => PayloadKind::Symbolic,
            Payload::Run(_) => PayloadKind::Run,
        }
    }

    pub fn samples(&self) -> Option<&Buffer> {
        match self {
            Payload::Samples(buffer) => Some(buffer),
            _ => None,
        }
    }

    pub fn symbolic(&self) -> Option<&SpectralSum> {
        match self {
            Payload::Symbolic(sum) => Some(sum),
            _ => None,
        }
    }

    pub fn run(self) -> Option<Run> {
        match self {
            Payload::Run(run) => Some(*run),
            _ => None,
        }
    }

    pub fn bytes(&self) -> usize {
        match self {
            Payload::Samples(b) => b.len() * b.width * size_of::<f64>(),
            Payload::Frames(f) => f.width * f.frames * f.bins * 2 * size_of::<f64>(),
            Payload::Symbolic(n) => {
                size_of::<SpectralSum>()
                    + n.lanes
                        .iter()
                        .map(|lane| {
                            lane.atoms.len() * size_of::<sva_formula::SpectralAtom>()
                                + (lane.series.len() + lane.modal.len()) * size_of::<SpectralSum>()
                        })
                        .sum::<usize>()
            }
            Payload::Run(run) => run.bytes(),
        }
    }

    pub fn answers(&self, expected: Expected) -> bool {
        match (self, expected) {
            (
                Payload::Samples(b),
                Expected::Samples {
                    rate,
                    width,
                    samples,
                },
            ) => b.rate == rate && b.width == width && b.len() == samples,
            (Payload::Frames(_), Expected::Frames) => true,
            (Payload::Symbolic(_), Expected::Symbolic) => true,
            (Payload::Run(run), Expected::Run { rate, width }) => {
                run.samples.rate == rate && run.samples.width == width
            }
            _ => false,
        }
    }
}

/// A hash carries the table version, so a bump retires every symbolic entry.
pub fn symbolic_key(src: Hash) -> Hash {
    mixed(src, &[0x73_79_6d_62_6f_6c_69_63])
}

/// The buffer's own key and the window it was read through.
pub fn frames_key(buffer: Hash, window: usize, hop: usize) -> Hash {
    mixed(
        buffer,
        &[window as u64, hop as u64, 0x66_72_61_6d_65_73_00_01],
    )
}

/// One closed form at one rate over one extent and width, scored or not: the label is part of
/// the value.
pub fn buffer_key(
    symbolic: Hash,
    rate: u32,
    extent: sva_samples::Extent,
    width: usize,
    score: sva_samples::AliasScore,
) -> Hash {
    mixed(
        symbolic,
        &[
            u64::from(rate),
            extent.start as u64,
            extent.len() as u64,
            width as u64,
            u64::from(score == sva_samples::AliasScore::Asked),
            0x62_75_66_66_65_72_00_01,
        ],
    )
}

/// Wherever the node is read from, and however far.
pub fn run_key(identity: Hash, rate: u32, width: usize) -> Hash {
    mixed(
        identity,
        &[u64::from(rate), width as u64, 0x72_75_6e_00_00_00_00_01],
    )
}

pub fn profiled_key(buffer: Hash, profile: &sva_samples::Profile) -> Hash {
    mixed(
        buffer,
        &[
            profile.precision_bits as u64,
            profile.ceiling_hz.to_bits(),
            u64::from(profile.lattice_hz),
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
