// Concern: one reading of a source's samples at a whole or fractional position | Non-concern: the kernel itself (reconstruct.rs), the positions a map names | IO: (Window, position) -> f64

use super::renderer::Map;
use super::tape::Window;
use crate::error::SampleError;
use crate::reconstruct::{kernel, taps};

/// Maps at most this fine keep one weight set per remainder; a fixed fraction keeps one.
const HELD_REMAINDERS: i128 = 1024;

/// One op's weights, kept across samples where its fractions repeat.
#[derive(Clone, Default)]
pub(super) struct Memo {
    sets: Vec<Vec<f64>>,
    scratch: Vec<f64>,
}

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

    fn weighed(&self, c: usize, floor: i64, weights: &[f64]) -> Result<f64, SampleError> {
        let span = taps(floor, weights.len() / 2);
        if let Some(limit) = self.limit
            && *span.end() >= limit
        {
            return Err(SampleError::ReadsAhead { at: *span.end() });
        }
        if let Some(held) = self.window.held(c, *span.start(), *span.end() + 1) {
            return Ok(held
                .iter()
                .zip(weights)
                .fold(0.0, |acc, (x, w)| acc + w * x));
        }
        let mut acc = 0.0;
        for (k, w) in span.zip(weights) {
            acc += w * self.sample(c, k)?;
        }
        Ok(acc)
    }

    pub(super) fn mapped(
        &self,
        (map, half_width): (Map, usize),
        n: i64,
        memo: &mut Memo,
        out: &mut [f64],
    ) -> Result<(), SampleError> {
        let (floor, rem) = map.at(n);
        if rem == 0 {
            for (c, slot) in out.iter_mut().enumerate() {
                *slot = self.sample(c, floor)?;
            }
            return Ok(());
        }
        let frac = rem as f64 / map.d as f64;
        let (kept, slot) = match map.a % map.d == 0 {
            true => (1, 0),
            false => (map.d, rem),
        };
        let weights = match kept <= HELD_REMAINDERS {
            true => {
                if memo.sets.len() != kept as usize {
                    memo.sets = vec![Vec::new(); kept as usize];
                }
                let set = &mut memo.sets[slot as usize];
                if set.is_empty() {
                    kernel(half_width).weights(frac, set);
                }
                &memo.sets[slot as usize]
            }
            false => {
                kernel(half_width).weights(frac, &mut memo.scratch);
                &memo.scratch
            }
        };
        for (c, slot) in out.iter_mut().enumerate() {
            *slot = self.weighed(c, floor, weights)?;
        }
        Ok(())
    }

    /// Position `whole + part`, `part` apart so its fraction keeps the bits a large `whole` would
    /// round away.
    pub(super) fn at(
        &self,
        (whole, part): (i64, f64),
        half_width: usize,
        memo: &mut Memo,
        out: &mut [f64],
    ) -> Result<(), SampleError> {
        let floor = part.floor();
        if !floor.is_finite() || floor.abs() >= 2f64.powi(62) {
            return Err(SampleError::UnreadablePosition);
        }
        let frac = part - floor;
        let floor = whole
            .checked_add(floor as i64)
            .ok_or(SampleError::UnreadablePosition)?;
        if frac == 0.0 {
            for (c, slot) in out.iter_mut().enumerate() {
                *slot = self.sample(c, floor)?;
            }
            return Ok(());
        }
        kernel(half_width).weights(frac, &mut memo.scratch);
        for (c, slot) in out.iter_mut().enumerate() {
            *slot = self.weighed(c, floor, &memo.scratch)?;
        }
        Ok(())
    }
}

/// `window` read at every sample of `over` through `map`, component by component.
pub fn resample(
    window: Window,
    (map, half_width): (Map, usize),
    over: crate::collapse::Extent,
    width: usize,
) -> Result<Vec<Vec<f64>>, SampleError> {
    let source = Source {
        window,
        limit: None,
    };
    let (mut memo, mut at) = (Memo::default(), vec![0.0; width.max(1)]);
    let mut planes = vec![Vec::with_capacity(over.len()); width.max(1)];
    for n in over.start..over.end {
        source.mapped((map, half_width), n, &mut memo, &mut at)?;
        for (plane, v) in planes.iter_mut().zip(&at) {
            plane.push(*v);
        }
    }
    Ok(planes)
}

/// `window` at one position in its own samples, whole or not.
pub fn read_at(
    window: Window,
    p: f64,
    half_width: usize,
    width: usize,
) -> Result<Vec<f64>, SampleError> {
    let mut out = vec![0.0; width.max(1)];
    Source {
        window,
        limit: None,
    }
    .at((0, p), half_width, &mut Memo::default(), &mut out)?;
    Ok(out)
}
