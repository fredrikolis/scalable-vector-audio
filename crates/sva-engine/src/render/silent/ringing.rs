// Concern: bounds a fixed biquad's impulse response from every lag on, and its sum | Non-concern: moving coefficients | IO: (Coeffs) -> Ringing, or none

use sva_samples::Coeffs;

const MAX_PERIOD: usize = 1 << 20;

/// `|h(n)| <= scale * ratio^((n - 2) / period)` for `n >= 2`, with `h(0)` and `h(1)` exact.
pub(super) struct Ringing {
    head: [f64; 2],
    scale: f64,
    ratio: f64,
    period: usize,
    pub(super) sum: f64,
}

impl Ringing {
    /// `(h(n), h(n-1)) = A^(n-2) (h(2), h(1))`, and `|A^(qK + r)| <= |A^K|^q max_r |A^r|`.
    pub(super) fn of(c: &Coeffs) -> Option<Ringing> {
        let h0 = c.b0;
        let h1 = c.b1 - c.a1 * h0;
        let h2 = c.b2 - c.a1 * h1 - c.a2 * h0;
        let a = [[-c.a1, -c.a2], [1.0, 0.0]];
        let mut power = [[1.0, 0.0], [0.0, 1.0]];
        let mut widest = 1.0f64;
        let mut period = 0;
        let ratio = loop {
            power = product(power, a);
            period += 1;
            let norm = norm(power);
            if !norm.is_finite() || period > MAX_PERIOD {
                return None;
            }
            if norm <= 0.5 {
                break norm;
            }
            widest = widest.max(norm);
        };
        let slack = 1.0 + 8.0 * period as f64 * f64::EPSILON;
        let scale = h2.abs().max(h1.abs()) * widest * slack;
        let ratio = ratio * slack;
        Some(Ringing {
            head: [h0.abs(), h1.abs()],
            scale,
            ratio,
            period,
            sum: h0.abs() + h1.abs() + scale * period as f64 / (1.0 - ratio),
        })
    }

    pub(super) fn from(&self, n: usize) -> f64 {
        let tail = |n: usize| {
            let steps = (n.max(2) - 2) / self.period;
            self.scale * self.ratio.powi(i32::try_from(steps).unwrap_or(i32::MAX))
        };
        match n {
            0 => self.head[0].max(self.head[1]).max(tail(2)),
            1 => self.head[1].max(tail(2)),
            n => tail(n),
        }
    }
}

fn product(p: [[f64; 2]; 2], a: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    let cell = |i: usize, j: usize| p[i][0] * a[0][j] + p[i][1] * a[1][j];
    [[cell(0, 0), cell(0, 1)], [cell(1, 0), cell(1, 1)]]
}

fn norm(p: [[f64; 2]; 2]) -> f64 {
    (p[0][0].abs() + p[0][1].abs()).max(p[1][0].abs() + p[1][1].abs())
}
