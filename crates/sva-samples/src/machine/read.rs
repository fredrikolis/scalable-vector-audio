// Concern: one reading of a source's stored samples at the whole index a map names | Non-concern: the positions a map names (renderer.rs) | IO: (Window, map, sample) -> f64 per component

use super::renderer::Map;
use super::tape::Window;
use crate::error::SampleError;

/// Samples of `window`, or `fresh`, before `limit` only: a node's own past is written up to
/// the sample being computed.
pub(super) struct Source<'a> {
    pub(super) window: Window<'a>,
    pub(super) limit: Option<i64>,
    pub(super) fresh: Fresh<'a>,
}

#[derive(Clone, Copy)]
pub(super) struct Fresh<'a> {
    pub(super) from: i64,
    pub(super) values: &'a [f64],
    pub(super) width: usize,
}

impl Source<'_> {
    fn sample(&self, c: usize, k: i64) -> Result<f64, SampleError> {
        #[cfg(test)]
        super::counts::READS.with(|n| n.set(n.get() + 1));
        if self.limit.is_some_and(|limit| k >= limit) {
            return Err(SampleError::ReadsAhead { at: k });
        }
        let Fresh {
            from,
            values,
            width,
        } = self.fresh;
        let fresh = k.checked_sub(from).and_then(|at| usize::try_from(at).ok());
        let fresh = fresh.filter(|_| self.limit.is_some());
        let held = match fresh {
            Some(at) => values
                .get(at * width..(at + 1) * width)
                .map(|v| super::part(v, c)),
            None => self.window.get(c, k),
        };
        held.ok_or(SampleError::ReadsAhead { at: k })
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
        let runs: Option<Vec<&[f64]>> = (0..w).map(|c| self.window.run(c, first, len)).collect();
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
