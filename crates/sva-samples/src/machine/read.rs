// Concern: one reading of a source's stored samples at the whole index a map names | Non-concern: the positions a map names (renderer.rs) | IO: (Window, map, sample) -> f64 per component

use super::renderer::Map;
use super::tape::Window;
use crate::error::SampleError;

/// Samples of `window` before `limit` only: a node's own past is written up to the sample
/// being computed.
pub(super) struct Source<'a> {
    pub(super) window: Window<'a>,
    pub(super) limit: Option<i64>,
}

impl Source<'_> {
    fn sample(&self, c: usize, k: i64) -> Result<f64, SampleError> {
        if self.limit.is_some_and(|limit| k >= limit) {
            return Err(SampleError::ReadsAhead { at: k });
        }
        self.window
            .get(c, k)
            .ok_or(SampleError::ReadsAhead { at: k })
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
