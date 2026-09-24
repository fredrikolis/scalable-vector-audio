// Concern: declares what a store holds under a content hash and how a key is built | Non-concern: any one store's medium and budget (disk.rs, memory.rs) | IO: (Hash) -> a payload + traces

mod disk;
mod entry_bytes;
mod evict;
mod label;
mod memory;
mod pack;
mod slots;
mod stats;
mod tiered;

pub use disk::{DiskCache, ENGINE_DIR_PREFIX, IO_NANOS_PER_BYTE};
pub use entry_bytes::{RawF64, SampleCodec};
pub use memory::MemoryCache;
pub use pack::{FORMAT as PACK_FORMAT, Medium, Pack, VecMedium};
pub use slots::{DEFAULT_SLOT_BYTES, Put, Slots};
pub(crate) use stats::Recording;
pub use stats::{CacheStats, Lookup, Outcome};
pub use sva_formula::Hash;
pub use tiered::Tiered;

use std::path::Path;
use std::time::Duration;

use sva_formula::SpectralSum;
use sva_samples::{Buffer, FilterTrace, Frames, Label};

/// A spectral sum, one collapse of it, or one analysis of that collapse.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    Samples(Box<Buffer>),
    Frames(Box<Frames>),
    Symbolic(Box<SpectralSum>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadKind {
    Samples,
    Frames,
    Symbolic,
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
}

/// Where a hit was answered from: this process's heap, a store that outlives it, or a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Memory,
    Persistent,
    Volatile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub payload: Payload,
    pub traces: Vec<FilterTrace>,
    /// FORMAT 9.3: the label is part of the value, so a hit answers with the cold run's.
    pub label: Option<Label>,
    pub tier: Tier,
}

impl Expected {
    pub fn kind(self) -> PayloadKind {
        match self {
            Expected::Samples { .. } => PayloadKind::Samples,
            Expected::Frames => PayloadKind::Frames,
            Expected::Symbolic => PayloadKind::Symbolic,
        }
    }
}

impl Payload {
    pub fn kind(&self) -> PayloadKind {
        match self {
            Payload::Samples(_) => PayloadKind::Samples,
            Payload::Frames(_) => PayloadKind::Frames,
            Payload::Symbolic(_) => PayloadKind::Symbolic,
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
            _ => false,
        }
    }
}

/// What a render spent on one value. `wasm32-unknown-unknown` has no clock at all, so a
/// render there measures nothing.
#[derive(Clone, Copy)]
pub struct Cost {
    #[cfg(not(target_arch = "wasm32"))]
    began: std::time::Instant,
}

impl Cost {
    pub fn begun() -> Cost {
        Cost {
            #[cfg(not(target_arch = "wasm32"))]
            began: std::time::Instant::now(),
        }
    }

    pub fn elapsed(self) -> Duration {
        #[cfg(not(target_arch = "wasm32"))]
        return self.began.elapsed();
        #[cfg(target_arch = "wasm32")]
        return Duration::ZERO;
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

/// One closed form at one rate, origin, length and width, scored or not: the label is part of
/// the value.
pub fn buffer_key(
    symbolic: Hash,
    rate: u32,
    origin_secs: f64,
    samples: usize,
    width: usize,
    score: sva_samples::AliasScore,
) -> Hash {
    mixed(
        symbolic,
        &[
            u64::from(rate),
            origin_secs.to_bits(),
            samples as u64,
            width as u64,
            u64::from(score == sva_samples::AliasScore::Asked),
            0x62_75_66_66_65_72_00_01,
        ],
    )
}

fn renamed(traces: &[FilterTrace], node: &str) -> Vec<FilterTrace> {
    traces
        .iter()
        .map(|t| FilterTrace {
            node: node.to_string(),
            ..t.clone()
        })
        .collect()
}

const ADDRESS_ROTATE: u32 = 17;

fn mixed(seed: Hash, parts: &[u64]) -> Hash {
    let mut lanes = sva_formula::Lanes::<ADDRESS_ROTATE>::from(seed);
    for part in parts {
        lanes.word(*part);
    }
    lanes.finish()
}

pub trait Cache: Sync {
    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry>;

    /// `load` with no side effect: no recency, promotion, fault or removal.
    fn peek(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry>;

    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>);

    fn holds(&self, key: Hash) -> bool;

    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool;

    fn sweep(&self);

    fn held_bytes(&self) -> u64;

    fn evicted_bytes(&self) -> u64;

    fn faults(&self) -> u64 {
        0
    }

    fn max_bytes(&self) -> u64;

    fn dir(&self) -> Option<&Path> {
        None
    }
}
