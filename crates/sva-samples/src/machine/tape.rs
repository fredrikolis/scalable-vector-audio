// Concern: one node's output over a trailing span of the grid, and a view of any span | Non-concern: what writes it, how far back a reader reaches | IO: (component, index) -> f64

use crate::buffer::Buffer;

/// Samples `[base, end)` of one node's output, indexed from the grid's first sample.
#[derive(Clone, Debug, PartialEq)]
pub struct Tape {
    planes: Vec<Vec<f64>>,
    base: usize,
}

impl Tape {
    pub fn new(width: usize, capacity: usize) -> Tape {
        Tape::starting_at(width, capacity, 0)
    }

    pub fn starting_at(width: usize, capacity: usize, base: usize) -> Tape {
        Tape {
            planes: (0..width.max(1))
                .map(|_| Vec::with_capacity(capacity))
                .collect(),
            base,
        }
    }

    pub fn width(&self) -> usize {
        self.planes.len()
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn capacity(&self) -> usize {
        self.planes[0].capacity()
    }

    pub fn end(&self) -> usize {
        self.base + self.planes[0].len()
    }

    pub fn window(&self) -> Window<'_> {
        Window {
            planes: &self.planes,
            base: self.base,
        }
    }

    pub fn since(&self, c: usize, from: usize) -> &[f64] {
        &self.planes[c][from - self.base..]
    }

    pub fn push(&mut self, c: usize, value: f64) {
        self.planes[c].push(value);
    }

    pub fn forget_before(&mut self, from: usize) {
        let gone = from.saturating_sub(self.base).min(self.planes[0].len());
        if gone == 0 {
            return;
        }
        for plane in &mut self.planes {
            plane.drain(..gone);
        }
        self.base += gone;
    }

    pub fn into_planes(self) -> Vec<Vec<f64>> {
        self.planes
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Window<'a> {
    planes: &'a [Vec<f64>],
    base: usize,
}

impl<'a> Window<'a> {
    pub fn of(buffer: &'a Buffer) -> Window<'a> {
        Window {
            planes: &buffer.planes,
            base: 0,
        }
    }

    /// A forgotten sample panics: some reader reaches further back than the tape keeps.
    pub fn at(&self, c: usize, k: i64) -> f64 {
        let Ok(k) = usize::try_from(k) else {
            return 0.0;
        };
        let j = k.checked_sub(self.base).unwrap_or_else(|| {
            panic!(
                "sample {k} was forgotten; this tape keeps from {}",
                self.base
            )
        });
        self.planes[c].get(j).copied().unwrap_or(0.0)
    }
}
