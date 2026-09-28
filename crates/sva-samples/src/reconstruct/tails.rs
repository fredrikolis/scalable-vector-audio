// Concern: bounds the aliases a windowed sinc passes | Non-concern: the table (table.rs) | IO: (beta, N, band) -> f64

use std::f64::consts::PI;

use super::i0;

const ALIASES: usize = 1 << 13;

/// Past the lobe `W^ = (2N/I0) sin(th)/th`.
pub(super) struct Tails {
    c: f64,
    p: f64,
    b: f64,
    n: f64,
    pub(super) lobe: f64,
}

impl Tails {
    pub(super) fn new(beta: f64, n: f64, lobe: f64) -> Tails {
        let norm = i0(beta);
        Tails {
            c: 1.0 / (PI * PI * n * norm),
            p: 1.0 / (PI * norm),
            b: lobe * lobe,
            n,
            lobe,
        }
    }

    /// The second mean value theorem.
    fn tail(&self, u: f64) -> f64 {
        self.c / u
    }

    /// Or by parts.
    fn unit(&self, u: f64) -> f64 {
        let parts = self.p * (1.0 / (PI * self.n) + self.b) / (u * (u + 1.0));
        (self.tail(u) + self.tail(u + 1.0)).min(parts)
    }

    fn aliases(&self, nu: f64) -> f64 {
        let near: f64 = (2..ALIASES)
            .map(|k| self.unit(k as f64 - nu - 0.5) + self.unit(k as f64 - 0.5))
            .sum();
        near + 2.0 * self.p * (1.0 / (PI * self.n) + self.b) / (ALIASES as f64 - 2.0)
    }

    /// Poisson summation over `K^ = rect * W^`.
    pub(super) fn in_band(&self, nu: f64) -> f64 {
        let own = self.tail(0.5 - nu) + self.tail(0.5);
        own + self.unit(0.5 - nu) + self.unit(0.5) + self.aliases(nu)
    }

    /// `K^(v) + K^(1-v)` is one, less tails.
    pub(super) fn above_band(&self, nu: f64) -> f64 {
        let halves = 1.0 + 2.0 * self.tail(self.lobe) + 2.0 * self.tail(0.5 + nu);
        let pair = self.tail(0.5 + nu) + self.tail(1.0);
        halves + pair + self.unit(0.5 + nu) + self.unit(1.0) + self.aliases(0.5)
    }
}
