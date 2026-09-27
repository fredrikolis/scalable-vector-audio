// Concern: bounds a fixed biquad's output from each grid instant on, over its input's bound | Non-concern: the impulse response itself (ringing.rs) | IO: (input bounds, instant) -> a bound

use sva_formula::NodeId;
use sva_samples::Coeffs;

use super::range::OP;
use super::ringing::Ringing;

/// `|y| <= |h| * |x|`: each earlier block of input meets only the response past the lags
/// between; each step's rounding runs through the recursion alone.
pub(super) struct Filtered {
    pub(super) x: NodeId,
    ringing: Ringing,
    recursion: Ringing,
    feed: f64,
    back: f64,
    lag: Vec<f64>,
    beyond: f64,
    largest: f64,
    slack: f64,
}

impl Filtered {
    pub(super) fn of(x: NodeId, c: &Coeffs) -> Option<Filtered> {
        let recursion = Coeffs {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            ..*c
        };
        let (ringing, recursion) = (Ringing::of(c)?, Ringing::of(&recursion)?);
        let negligible = (ringing.sum * 1e-18).max(f64::MIN_POSITIVE);
        let (reach, beyond) = ringing.reach(negligible);
        let lag = (0..reach).map(|g| ringing.window(g)).collect();
        Some(Filtered {
            x,
            beyond,
            ringing,
            recursion,
            feed: c.b0.abs() + c.b1.abs() + c.b2.abs(),
            back: c.a1.abs() + c.a2.abs(),
            lag,
            largest: 0.0,
            slack: 0.0,
        })
    }

    pub(super) fn start(&mut self, largest: f64) {
        self.largest = largest;
        let (feed, back) = (self.feed, self.back);
        self.slack = self.recursion.sum * OP * (feed + back * self.ringing.sum) * largest * 2.0;
    }

    pub(super) fn before(&self) -> f64 {
        self.ringing.sum * self.largest + self.slack
    }

    pub(super) fn at(&self, x: &[f64], j: usize) -> f64 {
        let near: f64 = (1..=j.min(self.lag.len()))
            .map(|g| self.lag[g - 1] * x[j - g])
            .sum();
        self.ringing.sum * x[j] + near + self.beyond * self.largest + self.slack
    }
}
