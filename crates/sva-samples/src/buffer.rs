// Concern: the one sample container: planar components over a span of the grid, grown, cut and viewed | Non-concern: producing or measuring them | IO: (component, index) -> f64

use std::borrow::Cow;
use std::ops::Range;

use crate::grid::Extent;

#[derive(Clone, Debug, PartialEq)]
pub struct Buffer {
    pub rate: u32,
    /// The grid sample the first of these stands at; sample 0 is t = 0.
    pub start: i64,
    pub planes: Vec<Vec<f64>>,
}

impl Buffer {
    pub fn silence(rate: u32, width: usize, len: usize) -> Buffer {
        Buffer {
            rate,
            start: 0,
            planes: vec![vec![0.0; len]; width],
        }
    }

    pub fn empty(rate: u32, width: usize, capacity: usize, start: i64) -> Buffer {
        Buffer {
            rate,
            start,
            planes: (0..width.max(1))
                .map(|_| Vec::with_capacity(capacity))
                .collect(),
        }
    }

    pub fn mono(rate: u32, samples: Vec<f64>) -> Buffer {
        Buffer {
            rate,
            start: 0,
            planes: vec![samples],
        }
    }

    pub fn of_planes(rate: u32, planes: Vec<Vec<f64>>) -> Buffer {
        let len = planes.iter().map(Vec::len).min().unwrap_or(0);
        Buffer {
            rate,
            start: 0,
            planes: planes
                .into_iter()
                .map(|mut p| {
                    p.truncate(len);
                    p
                })
                .collect(),
        }
    }

    pub fn width(&self) -> usize {
        self.planes.len()
    }

    pub fn plane(&self, c: usize) -> &[f64] {
        &self.planes[c]
    }

    pub fn plane_mut(&mut self, c: usize) -> &mut [f64] {
        &mut self.planes[c]
    }

    pub fn slices(&self) -> Vec<&[f64]> {
        self.planes.iter().map(Vec::as_slice).collect()
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

    pub fn origin_secs(&self) -> f64 {
        self.start as f64 / f64::from(self.rate)
    }

    pub fn end(&self) -> i64 {
        self.start + self.len() as i64
    }

    pub fn extent(&self) -> Extent {
        Extent::new(self.start, self.end())
    }

    pub fn capacity(&self) -> usize {
        self.planes[0].capacity()
    }

    pub fn push(&mut self, c: usize, value: f64) {
        self.planes[c].push(value);
    }

    pub fn forget_before(&mut self, from: i64) {
        let gone = (from - self.start).clamp(0, self.len() as i64) as usize;
        if gone == 0 {
            return;
        }
        for plane in &mut self.planes {
            plane.drain(..gone);
        }
        self.start += gone as i64;
    }

    /// A node zero outside `support`.
    pub fn within(&self, support: Extent) -> Window<'_> {
        Window {
            planes: &self.planes,
            base: self.start,
            support,
            period: None,
            offset: 0,
        }
    }

    /// These samples over `over`, a node that is zero outside `support`.
    pub fn over(&self, over: Extent, support: Extent) -> Buffer {
        let window = self.within(support);
        let planes = match !over.is_empty() && self.extent().intersect(over) == over {
            true => self
                .planes
                .iter()
                .map(|p| p[(over.start - self.start) as usize..][..over.len()].to_vec())
                .collect(),
            false => (0..self.width())
                .map(|c| (over.start..over.end).map(|n| window.at(c, n)).collect())
                .collect(),
        };
        let mut out = Buffer::of_planes(self.rate, planes);
        out.start = over.start;
        out
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

/// A view of held samples; a periodic value's samples are held over one period from 0, and
/// sample `k` is sample `k mod period`.
#[derive(Clone, Copy, Debug)]
pub struct Window<'a> {
    planes: &'a [Vec<f64>],
    base: i64,
    support: Extent,
    period: Option<i64>,
    /// Sample `k` of the view is sample `k + offset` of what it views.
    offset: i64,
}

impl<'a> Window<'a> {
    pub fn folded(self, period: Option<i64>) -> Window<'a> {
        Window { period, ..self }
    }

    pub fn shifted(self, by: i64) -> Window<'a> {
        Window {
            offset: self.offset + by,
            ..self
        }
    }

    fn fold(&self, k: i64) -> i64 {
        let k = k.saturating_add(self.offset);
        match self.period {
            Some(n) if self.support.contains(k) => k.rem_euclid(n),
            _ => k,
        }
    }

    /// `None` for a sample inside the support this window does not hold.
    pub fn get(&self, c: usize, k: i64) -> Option<f64> {
        let k = self.fold(k);
        let plane = &self.planes[c];
        let held = k
            .checked_sub(self.base)
            .and_then(|at| usize::try_from(at).ok())
            .and_then(|at| plane.get(at));
        match held {
            Some(v) => Some(*v),
            None => (!self.support.contains(k)).then_some(0.0),
        }
    }

    /// Samples `[k, k + len)` of component `c`, where held unfolded.
    pub fn run(&self, c: usize, k: i64, len: usize) -> Option<&'a [f64]> {
        if self.period.is_some() {
            return None;
        }
        let at = usize::try_from(k.checked_add(self.offset)?.checked_sub(self.base)?).ok()?;
        self.planes.get(c)?.get(at..at.checked_add(len)?)
    }

    /// A sample not held inside the support is a reader past its extent.
    pub fn at(&self, c: usize, k: i64) -> f64 {
        let k = self.fold(k);
        let plane = &self.planes[c];
        if let Some(held) = k
            .checked_sub(self.base)
            .and_then(|at| usize::try_from(at).ok())
            .and_then(|at| plane.get(at))
        {
            return *held;
        }
        if !self.support.contains(k) {
            return 0.0;
        }
        panic!(
            "sample {k} is not held: this node holds [{}, {}) and is nonzero over [{}, {})",
            self.base,
            self.base + plane.len() as i64,
            self.support.start,
            self.support.end
        )
    }
}
