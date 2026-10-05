// Concern: what a block reads, another node's samples or its own past, at the whole index a map names | Non-concern: the positions a map names | IO: (SampleView, map, sample) -> f64 per component

use super::renderer::{Map, Slot};
use crate::buffer::SampleView;
use crate::error::SampleError;
use crate::grid::Grid;

/// What a block reads: other nodes' samples, this node's own past before the block, and the
/// grid it steps on.
pub(super) struct Here<'a> {
    pub(super) reads: &'a [SampleView<'a>],
    pub(super) own: SampleView<'a>,
    pub(super) grid: Grid,
}

impl<'a> Here<'a> {
    pub(super) fn source(&self, slot: Slot, n: i64) -> Source<'a> {
        match slot {
            Slot::Read(id) => Source {
                view: self.reads[id.0 as usize],
                limit: None,
            },
            Slot::Own => Source {
                view: self.own,
                limit: Some(n),
            },
        }
    }
}

pub(super) struct Source<'a> {
    pub(super) view: SampleView<'a>,
    pub(super) limit: Option<i64>,
}

impl Source<'_> {
    fn sample(&self, c: usize, k: i64) -> Result<f64, SampleError> {
        if self.limit.is_some_and(|limit| k >= limit) {
            return Err(SampleError::ReadsAhead { at: k });
        }
        self.view.get(c, k).ok_or(SampleError::ReadsAhead { at: k })
    }

    /// Samples `n` on, copied as one run where `map` shifts onto held ones; `false` else.
    pub(super) fn copied(&self, map: Map, n: i64, (out, w): (&mut [f64], usize)) -> bool {
        let len = out.len() / w;
        let Some(by) = map.moved() else {
            return false;
        };
        let Some(first) = n.checked_add(by) else {
            return false;
        };
        if self.limit.is_some() && by >= 0 {
            return false;
        }
        let runs: Option<Vec<&[f64]>> = (0..w).map(|c| self.view.run(c, first, len)).collect();
        let Some(runs) = runs else {
            return false;
        };
        for (c, run) in runs.into_iter().enumerate() {
            for (sample, v) in out.chunks_exact_mut(w).zip(run) {
                sample[c] = *v;
            }
        }
        true
    }

    pub(super) fn mapped(&self, map: Map, n: i64, out: &mut [f64]) -> Result<(), SampleError> {
        self.nearest(map.at(n), out)
    }

    pub(super) fn nearest(&self, k: i64, out: &mut [f64]) -> Result<(), SampleError> {
        for (c, slot) in out.iter_mut().enumerate() {
            *slot = self.sample(c, k)?;
        }
        Ok(())
    }
}
