// Concern: what the tier answers of a node, and where a disk entry lays its samples | Non-concern: reading them, the bytes (codec.rs) | IO: Header -> extents

use sva_formula::{Codomain, Hash};
use sva_samples::{Extent, Grid, Label};

#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub key: Hash,
    pub identity: Hash,
    pub label: Label,
    pub width: u8,
    pub codomain: Codomain,
    pub rate: Option<u32>,
    pub grid: Grid,
    pub support: Extent,
    /// The most seconds a read under it moved to land on a sample.
    pub moved: f64,
    pub readable: bool,
    pub sampled: bool,
    pub(crate) held: Vec<Extent>,
}

impl Stored {
    pub(crate) fn extents(&self) -> &[Extent] {
        &self.held
    }

    pub(crate) fn holds(&self, over: Extent) -> bool {
        let mut from = over.start;
        for part in &self.held {
            if from >= over.end || part.start > from {
                break;
            }
            from = from.max(part.end);
        }
        from >= over.end
    }

    pub(crate) fn holding(&self, mut held: Vec<Extent>) -> Stored {
        held.sort_by_key(|e| e.start);
        Stored {
            held,
            ..self.clone()
        }
    }
}

/// Only the disk tier and the memory tier read a layout. What it holds is its layout's alone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Header {
    stored: Stored,
    samples: Samples,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Samples {
    #[default]
    None,
    /// Its `n` is the file's `n - shift`.
    Entry {
        file: Hash,
        runs: Vec<Laid>,
        shift: i64,
    },
    Staged {
        file: Hash,
        chunks: Vec<(String, Extent)>,
        shift: i64,
    },
    /// As written: sample `n` is `key`'s sample `n + by`.
    Of { key: Hash, by: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Laid {
    pub(crate) rate: u32,
    pub(crate) start: i64,
    pub(crate) width: usize,
    pub(crate) len: usize,
    /// Its first byte.
    pub(crate) at: u64,
    pub(crate) sums: Vec<u64>,
}

impl Laid {
    pub(crate) fn extent(&self) -> Extent {
        Extent::new(self.start, self.start + self.len as i64)
    }
}

impl Samples {
    fn extents(&self) -> Vec<Extent> {
        match self {
            Samples::None | Samples::Of { .. } => Vec::new(),
            Samples::Entry { runs, shift, .. } => runs
                .iter()
                .map(|run| run.extent().shifted(*shift))
                .collect(),
            Samples::Staged { chunks, shift, .. } => {
                chunks.iter().map(|(_, e)| e.shifted(*shift)).collect()
            }
        }
    }
}

impl Header {
    pub(crate) fn new(stored: Stored, samples: Samples) -> Header {
        let stored = stored.holding(samples.extents());
        Header { stored, samples }
    }

    pub(crate) fn stored(&self) -> &Stored {
        &self.stored
    }

    pub(crate) fn samples(&self) -> &Samples {
        &self.samples
    }

    pub(crate) fn into_parts(self) -> (Stored, Samples) {
        (self.stored, self.samples)
    }

    pub(crate) fn refers(&self) -> bool {
        matches!(self.samples, Samples::Of { .. })
    }
}
