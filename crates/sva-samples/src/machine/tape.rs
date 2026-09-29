// Concern: one node's output over a trailing span of the grid, and a view of any span | Non-concern: what writes it, how far back a reader reaches | IO: (component, index) -> f64

use crate::buffer::Buffer;
use crate::collapse::Extent;

/// Samples `[base, end)` of one node's output; it is silent before `origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct Tape {
    planes: Vec<Vec<f64>>,
    base: i64,
    origin: i64,
}

impl Tape {
    pub fn new(width: usize, capacity: usize, origin: i64) -> Tape {
        Tape {
            planes: (0..width.max(1))
                .map(|_| Vec::with_capacity(capacity))
                .collect(),
            base: origin,
            origin,
        }
    }

    pub fn width(&self) -> usize {
        self.planes.len()
    }

    pub fn base(&self) -> i64 {
        self.base
    }

    pub fn origin(&self) -> i64 {
        self.origin
    }

    pub fn planes(&self) -> &[Vec<f64>] {
        &self.planes
    }

    pub fn capacity(&self) -> usize {
        self.planes[0].capacity()
    }

    pub fn end(&self) -> i64 {
        self.base + self.planes[0].len() as i64
    }

    pub fn window(&self) -> Window<'_> {
        self.within(Extent::from(self.origin))
    }

    pub fn within(&self, support: Extent) -> Window<'_> {
        Window {
            planes: &self.planes,
            base: self.base,
            support,
            period: None,
            offset: 0,
        }
    }

    pub fn since(&self, c: usize, from: i64) -> &[f64] {
        &self.planes[c][(from - self.base) as usize..]
    }

    pub fn push(&mut self, c: usize, value: f64) {
        self.planes[c].push(value);
    }

    /// Everything from `end` on dropped.
    pub fn cut(&mut self, end: i64) {
        let kept = (end - self.base).clamp(0, self.planes[0].len() as i64) as usize;
        for plane in &mut self.planes {
            plane.truncate(kept);
        }
    }

    pub fn forget_before(&mut self, from: i64) {
        let gone = (from - self.base).clamp(0, self.planes[0].len() as i64) as usize;
        if gone == 0 {
            return;
        }
        for plane in &mut self.planes {
            plane.drain(..gone);
        }
        self.base += gone as i64;
    }

    pub fn into_planes(self) -> Vec<Vec<f64>> {
        self.planes
    }

    pub fn into_buffer(self, rate: u32) -> Buffer {
        let mut out = Buffer::of_planes(rate, self.planes);
        out.start = self.base;
        out
    }
}

/// A buffer held whole, silent before it starts.
impl From<Buffer> for Tape {
    fn from(buffer: Buffer) -> Tape {
        Tape {
            planes: buffer.planes,
            base: buffer.start,
            origin: buffer.start,
        }
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
    pub fn of(buffer: &'a Buffer, support: Extent) -> Window<'a> {
        Window {
            planes: &buffer.planes,
            base: buffer.start,
            support,
            period: None,
            offset: 0,
        }
    }

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
