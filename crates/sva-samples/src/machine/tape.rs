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

#[derive(Clone, Copy, Debug)]
pub struct Window<'a> {
    planes: &'a [Vec<f64>],
    base: i64,
    support: Extent,
}

impl<'a> Window<'a> {
    pub fn of(buffer: &'a Buffer, support: Extent) -> Window<'a> {
        Window {
            planes: &buffer.planes,
            base: buffer.start,
            support,
        }
    }

    /// `None` for a sample inside the support this window does not hold.
    pub fn get(&self, c: usize, k: i64) -> Option<f64> {
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

    /// Samples `[from, to)` where this window holds every one of them.
    pub fn held(&self, c: usize, from: i64, to: i64) -> Option<&'a [f64]> {
        let plane = &self.planes[c];
        let start = usize::try_from(from.checked_sub(self.base)?).ok()?;
        let end = usize::try_from(to.checked_sub(self.base)?).ok()?;
        plane.get(start..end)
    }

    /// A sample not held inside the support is a reader past its extent: no value answers it.
    pub fn at(&self, c: usize, k: i64) -> f64 {
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
