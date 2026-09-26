// Concern: declares what a store holds under a content hash and how a key is built | Non-concern: what the store keeps and evicts (store.rs) | IO: (Hash) -> a payload

mod stats;
mod store;

pub use stats::{CacheStats, Lookup, Outcome};
pub(crate) use stats::{Lens, Recording};
pub use store::{Cache, DEFAULT_CACHE_BYTES, PrunePolicy};
pub use sva_formula::Hash;

use sva_formula::SpectralSum;
use sva_samples::{Buffer, Frames, Label};

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

const ADDRESS_ROTATE: u32 = 17;

pub(crate) fn mixed(seed: Hash, parts: &[u64]) -> Hash {
    let mut lanes = sva_formula::Lanes::<ADDRESS_ROTATE>::from(seed);
    for part in parts {
        lanes.word(*part);
    }
    lanes.finish()
}
