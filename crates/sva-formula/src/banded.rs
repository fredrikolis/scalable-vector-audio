// Concern: which terms of a series an instant sums, its carriers under a ceiling | Non-concern: choosing one (truncate.rs), evaluating a term | IO: (slope, offset at t) -> indices

use crate::closed_form::{Bound, Series};
use crate::spectral_sum::SpectralSum;

/// Term `k` turns at `k*slope(t) + offset(t)` rad/s; an instant sums those under `omega`.
#[derive(Clone, Debug, PartialEq)]
pub struct Banded {
    pub series: Series,
    pub slope: SpectralSum,
    pub offset: SpectralSum,
    pub omega: f64,
    pub most: i64,
    pub widest: i64,
    pub reach: f64,
    pub dropped_db: f64,
}

impl Banded {
    pub fn within(&self, slope: f64, offset: f64) -> Option<(i64, i64)> {
        let sounds = |k: i64| (k as f64 * slope + offset).abs() < self.omega;
        let lo = self.series.lo;
        let hi = match self.series.hi {
            Bound::Finite(n) => n,
            Bound::Infinite => i64::MAX,
        };
        let (mut from, mut to) = match slope == 0.0 {
            true if offset.abs() < self.omega => (lo, hi),
            true => return None,
            false => {
                let ends = [
                    (-self.omega - offset) / slope,
                    (self.omega - offset) / slope,
                ];
                let index = |x: f64| x.clamp(-9e18, 9e18) as i64;
                (
                    index(ends[0].min(ends[1]).ceil()),
                    index(ends[0].max(ends[1]).floor()),
                )
            }
        };
        if slope != 0.0 {
            while from <= to && !sounds(from) {
                from += 1;
            }
            while from <= to && !sounds(to) {
                to -= 1;
            }
            while from <= to && from > lo && sounds(from - 1) {
                from -= 1;
            }
            while from <= to && to < hi && sounds(to + 1) {
                to += 1;
            }
        }
        let from = from.max(lo);
        let to = to.min(hi).min(from.saturating_add(self.most - 1));
        (from <= to).then_some((from, to))
    }
}
