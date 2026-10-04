// Concern: the collapse surface: a form's rows, its value at an instant, the grid's extents | Non-concern: its spectral sum (sva-formula), measuring samples (measure/) | IO: (&ClosedForm, rate) -> Rows

mod active;
mod addends;
mod atoms;
mod blocks;
mod lines;
mod point;
mod reading;
pub mod run;
mod tail;
mod truncate;

use sva_formula::{Body, C64, SpectralSum};

use crate::error::CollapseError;

pub use blocks::Rows;
pub use lines::summed_bounds;
pub(crate) use point::Shared;
pub use point::{At, Refs, crop_gain, lane_of, shoulders, unary};
pub use truncate::{
    Audible, dropped_db, spectral_sum as truncate_spectral_sum,
    spectral_sum_read as truncate_spectral_sum_read, written as truncate_written,
    written_with as truncate_written_with,
};

pub fn eval_spectral_sum_at(
    sum: &SpectralSum,
    component: usize,
    at: At,
) -> Result<C64, CollapseError> {
    point::eval_spectral_sum_on(sum, component, at)
}

/// One written closed form's value at one instant, every `Body::Node` in it answered by the
/// caller holding the graph.
pub fn eval_written_at(
    body: &Body,
    component: usize,
    at: At,
    refs: &dyn Refs,
) -> Result<C64, CollapseError> {
    point::eval_body_on(body, component, at, refs)
}

/// Samples `[start, end)` of the one grid every node is read on, whose sample 0 is t = 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extent {
    pub start: i64,
    pub end: i64,
}

impl Extent {
    /// Every sample of the grid: `i64::MIN` and `i64::MAX` stand for no edge at all.
    pub const EVERYWHERE: Extent = Extent {
        start: i64::MIN,
        end: i64::MAX,
    };

    pub const NOWHERE: Extent = Extent { start: 0, end: 0 };

    pub fn new(start: i64, end: i64) -> Extent {
        assert!(start <= end, "an extent [{start}, {end}) runs backwards");
        Extent { start, end }
    }

    pub fn from(start: i64) -> Extent {
        Extent::new(start, i64::MAX)
    }

    pub fn is_bounded(&self) -> bool {
        self.start != i64::MIN && self.end != i64::MAX
    }

    pub fn contains(&self, n: i64) -> bool {
        self.start <= n && n < self.end
    }

    pub fn intersect(self, other: Extent) -> Extent {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        match start < end {
            true => Extent { start, end },
            false => Extent::NOWHERE,
        }
    }

    pub fn hull(self, other: Extent) -> Extent {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other,
            (_, true) => self,
            _ => Extent {
                start: self.start.min(other.start),
                end: self.end.max(other.end),
            },
        }
    }

    /// Every sample moved `by` later; an unbounded edge stays unbounded.
    pub fn shifted(self, by: i64) -> Extent {
        if self.is_empty() {
            return self;
        }
        let edge = |n: i64| match n {
            i64::MIN | i64::MAX => n,
            n => n.saturating_add(by).clamp(i64::MIN + 1, i64::MAX - 1),
        };
        Extent {
            start: edge(self.start),
            end: edge(self.end),
        }
    }

    pub fn secs(rate: u32, start_secs: f64, end_secs: f64) -> Extent {
        let at = |secs: f64| (secs * f64::from(rate)).round() as i64;
        Extent::new(at(start_secs), at(end_secs))
    }

    pub fn len(&self) -> usize {
        debug_assert!(self.is_bounded(), "an unbounded extent has no length");
        (self.end - self.start) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }

    pub fn start_secs(&self, rate: u32) -> f64 {
        self.start as f64 / f64::from(rate)
    }

    pub fn span_secs(&self, rate: u32) -> f64 {
        self.len() as f64 / f64::from(rate)
    }
}
