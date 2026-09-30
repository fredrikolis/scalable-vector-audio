// Concern: what a lookup answers of a stored node: its meta and where its samples lie | Non-concern: reading those samples, the bytes (codec.rs) | IO: (identity, rate) -> key; Stored -> extents

use sva_formula::{Codomain, Hash};
use sva_samples::{Extent, Grid, Label};

#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    pub key: Hash,
    pub label: Label,
    pub width: u8,
    pub codomain: Codomain,
    pub rate: Option<u32>,
    pub grid: Grid,
    pub support: Extent,
    /// Flops it and all under it cost.
    pub priced: u128,
    /// The most seconds a read under it moved to land on a sample.
    pub moved: f64,
    pub readable: bool,
    pub(crate) samples: Samples,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Samples {
    #[default]
    None,
    Entry {
        file: Hash,
        runs: Vec<Laid>,
    },
    Staged(Vec<(String, Extent)>),
}

/// `at` is its first byte in the entry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Laid {
    pub(crate) rate: u32,
    pub(crate) start: i64,
    pub(crate) width: usize,
    pub(crate) len: usize,
    pub(crate) at: u64,
    pub(crate) sums: Vec<u64>,
}

impl Laid {
    pub(crate) fn extent(&self) -> Extent {
        Extent::new(self.start, self.start + self.len as i64)
    }
}

impl Stored {
    pub(crate) fn extents(&self) -> Vec<Extent> {
        let mut out: Vec<Extent> = match &self.samples {
            Samples::None => Vec::new(),
            Samples::Entry { runs, .. } => runs.iter().map(Laid::extent).collect(),
            Samples::Staged(chunks) => chunks.iter().map(|(_, e)| *e).collect(),
        };
        out.sort_by_key(|e| e.start);
        out
    }

    pub(crate) fn holds(&self, over: Extent) -> bool {
        let mut from = over.start;
        for part in self.extents() {
            if from >= over.end || part.start > from {
                break;
            }
            from = from.max(part.end);
        }
        from >= over.end
    }
}

/// Keyed by its source's identity, the rate and the profile.
pub(crate) fn node_key(identity: Hash, rate: u32, profile: &sva_samples::Profile) -> Hash {
    super::mixed(
        identity,
        &[
            u64::from(rate),
            profile.precision_bits as u64,
            profile.ceiling_hz.to_bits(),
            0x6e_6f_64_65_00_00_00_01,
        ],
    )
}
