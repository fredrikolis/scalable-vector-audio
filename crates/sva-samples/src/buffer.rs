// Concern: holds one collapse's samples as planar f64 components on a rate and origin | Non-concern: producing them (collapse.rs), measuring one (measure/) | IO: (component, index) -> f64

use std::borrow::Cow;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq)]
pub struct Buffer {
    pub rate: u32,
    pub origin_secs: f64,
    pub width: usize,
    pub planes: Vec<Vec<f64>>,
}

impl Buffer {
    pub fn silence(rate: u32, width: usize, len: usize) -> Buffer {
        Buffer {
            rate,
            origin_secs: 0.0,
            width,
            planes: vec![vec![0.0; len]; width],
        }
    }

    pub fn mono(rate: u32, samples: Vec<f64>) -> Buffer {
        Buffer {
            rate,
            origin_secs: 0.0,
            width: 1,
            planes: vec![samples],
        }
    }

    pub fn of_planes(rate: u32, planes: Vec<Vec<f64>>) -> Buffer {
        let len = planes.iter().map(Vec::len).min().unwrap_or(0);
        Buffer {
            rate,
            origin_secs: 0.0,
            width: planes.len(),
            planes: planes
                .into_iter()
                .map(|mut p| {
                    p.truncate(len);
                    p
                })
                .collect(),
        }
    }

    pub fn plane(&self, c: usize) -> &[f64] {
        &self.planes[c]
    }

    pub fn plane_mut(&mut self, c: usize) -> &mut [f64] {
        &mut self.planes[c]
    }

    pub fn len(&self) -> usize {
        self.planes.iter().map(Vec::len).min().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn at(&self, c: usize, i: usize) -> f64 {
        self.planes[c][i]
    }

    /// The samples `over` covers, nearest grid point at each end, held to what this holds.
    pub fn span_of(&self, over: crate::Horizon) -> Range<usize> {
        let rate = f64::from(self.rate);
        let at = |secs: f64| {
            (((secs - self.origin_secs) * rate).round().max(0.0) as usize).min(self.len())
        };
        let start = at(over.start_secs);
        let end = match over.end_secs.is_finite() {
            true => at(over.end_secs).max(start),
            false => self.len(),
        };
        start..end
    }

    pub fn window(&self, c: usize, range: Range<usize>) -> Cow<'_, [f64]> {
        let plane = self.plane(c);
        if range.end <= plane.len() {
            return Cow::Borrowed(&plane[range]);
        }
        let mut out = vec![0.0; range.len()];
        let held = plane.len().clamp(range.start, range.end) - range.start;
        out[..held].copy_from_slice(&plane[range.start.min(plane.len())..][..held]);
        Cow::Owned(out)
    }

    /// The only narrowing in the crate: a WAV file and the wasm boundary are both f32.
    pub fn as_f32(&self, c: usize) -> Vec<f32> {
        self.plane(c).iter().map(|&x| x as f32).collect()
    }
}
