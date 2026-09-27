// Concern: bounds a fixed biquad's impulse response from every lag on, and its sum | Non-concern: moving coefficients | IO: (Coeffs) -> Ringing, or none

use sva_samples::Coeffs;

use super::envelope::STEP;

const MAX_PERIOD: usize = 1 << 20;

const MAX_STEPPED: usize = 1 << 20;

const MAX_REACH: usize = 1 << 14;

/// `|h(n)| <= scale * ratio^((n - 2) / period)` for `n >= 2`; `sum` bounds `sum |h(n)|`,
/// `windows[g]` any `STEP` consecutive `|h(n)|` from lag `g * STEP + 1` on.
pub(crate) struct Ringing {
    head: [f64; 2],
    scale: f64,
    ratio: f64,
    period: usize,
    pub(crate) sum: f64,
    windows: Vec<f64>,
}

impl Ringing {
    /// `(h(n), h(n-1)) = A^(n-2) (h(2), h(1))`, and `|A^(qK + r)| <= |A^K|^q max_r |A^r|`.
    pub(crate) fn of(c: &Coeffs) -> Option<Ringing> {
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
        let mut held = Ringing {
            head: [h0.abs(), h1.abs()],
            scale,
            ratio,
            period,
            sum: h0.abs() + h1.abs() + scale * period as f64 / (1.0 - ratio),
            windows: Vec::new(),
        };
        let spread = widest * slack * period as f64 / (1.0 - ratio);
        held.stepped(c, spread);
        Some(held)
    }

    fn rest(&self, n: usize) -> f64 {
        let steps = (n.max(2) - 2) / self.period;
        let from = self.ratio.powi(i32::try_from(steps).unwrap_or(i32::MAX));
        self.scale * self.period as f64 * from / (1.0 - self.ratio)
    }

    /// Steps `h` out until the rest is a millionth of the sum, each term within the rounding
    /// the recursion spreads by `spread`.
    fn stepped(&mut self, c: &Coeffs, spread: f64) {
        let x = |at: usize| match at {
            0 => c.b0,
            1 => c.b1,
            2 => c.b2,
            _ => 0.0,
        };
        let mut h = vec![c.b0];
        let (mut total, mut largest) = (c.b0.abs(), c.b0.abs());
        while h.len() < MAX_STEPPED {
            let n = h.len();
            let before = if n >= 2 { h[n - 2] } else { 0.0 };
            let next = x(n) - c.a1 * h[n - 1] - c.a2 * before;
            total += next.abs();
            largest = largest.max(next.abs());
            h.push(next);
            if n >= 2 && self.rest(n + 1) <= total * 1e-6 {
                break;
            }
        }
        let n = h.len();
        let each = spread * 8.0 * f64::EPSILON * (c.a1.abs() + c.a2.abs()) * largest * 1.0001;
        let tail = self.rest(n);
        self.sum = self
            .sum
            .min((total + n as f64 * each) * (1.0 + 1e-9) + tail);
        let mut prefix = vec![0.0f64; n + 1];
        for (at, v) in h.iter().enumerate() {
            prefix[at + 1] = prefix[at] + v.abs();
        }
        let window = |m: usize| match m + STEP <= n {
            true => prefix[m + STEP] - prefix[m] + STEP as f64 * each,
            false => prefix[n] - prefix[m.min(n)] + (m + STEP - n) as f64 * each + tail,
        };
        let mut windows = vec![0.0f64; n / STEP + 1];
        let mut widest = self.rest(n);
        for m in (1..=n).rev() {
            widest = widest.max(window(m) * (1.0 + 1e-9));
            if (m - 1) % STEP == 0 {
                windows[(m - 1) / STEP] = widest;
            }
        }
        self.windows = windows;
    }

    pub(crate) fn window(&self, g: usize) -> f64 {
        let loose = STEP as f64 * self.from(g * STEP + 1);
        match self.windows.get(g) {
            Some(held) => held.min(loose),
            None => loose,
        }
    }

    /// The first lag block from which all later ones sum under `negligible`, and that sum:
    /// past the stepped blocks, `period / STEP + 1` blocks to each power of `ratio`.
    pub(crate) fn reach(&self, negligible: f64) -> (usize, f64) {
        let stepped = self.windows.len();
        let mut suffix = vec![0.0f64; stepped + 1];
        for i in (0..stepped).rev() {
            suffix[i] = suffix[i + 1] + self.window(i);
        }
        let beyond = |g: usize| {
            let from = g.max(stepped).max(1);
            let power = (from * STEP - 1) / self.period;
            let ratio = self.ratio.powi(i32::try_from(power).unwrap_or(i32::MAX));
            let run = self.scale * (self.period + STEP) as f64 * ratio / (1.0 - self.ratio);
            (suffix[g.min(stepped)] + run) * (1.0 + 1e-9)
        };
        let g = (0..MAX_REACH)
            .find(|g| beyond(*g) <= negligible)
            .unwrap_or(MAX_REACH);
        (g, beyond(g))
    }

    pub(crate) fn from(&self, n: usize) -> f64 {
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
