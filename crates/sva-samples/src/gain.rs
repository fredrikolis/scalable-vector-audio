// Concern: a proven bound on the most a biquad scales an input by | Non-concern: designing it (biquad.rs) | IO: (&Coeffs) -> sum |h|

use crate::biquad::{Coeffs, State};

const U: f64 = f64::EPSILON / 2.0;
const DIRECT: usize = 1 << 12;

type Matrix = [[f64; 2]; 2];

fn gamma(k: f64) -> f64 {
    k * U / (1.0 - k * U)
}

fn norm(m: Matrix) -> f64 {
    (m[0][0].abs() + m[0][1].abs()).max(m[1][0].abs() + m[1][1].abs())
}

fn times(x: Matrix, y: Matrix) -> Matrix {
    [
        [
            x[0][0] * y[0][0] + x[0][1] * y[1][0],
            x[0][0] * y[0][1] + x[0][1] * y[1][1],
        ],
        [
            x[1][0] * y[0][0] + x[1][1] * y[1][0],
            x[1][0] * y[0][1] + x[1][1] * y[1][1],
        ],
    ]
}

/// Rounding carried through each step; the tail `2 sum |A^m| |state|` once `|A^M| <= 1/2`.
pub fn l1(c: &Coeffs) -> Option<f64> {
    let (a1, a2) = (c.a1.abs(), c.a2.abs());
    let mut state = State::default();
    let (mut sum, mut err, mut e1, mut e2) = (0.0, 0.0, 0.0, 0.0);
    for n in 0..DIRECT {
        let x = f64::from(u8::from(n == 0));
        let [x1, x2, y1, y2] = state.held();
        let terms = (c.b0 * x).abs()
            + (c.b1 * x1).abs()
            + (c.b2 * x2).abs()
            + a1 * y1.abs()
            + a2 * y2.abs();
        let e = a1 * e1 + a2 * e2 + gamma(5.0) * terms;
        sum += state.step(c, x).abs();
        err += e;
        (e2, e1) = (e1, e);
    }
    let [_, _, y1, y2] = state.held();
    let held = (y1.abs() + e1).max(y2.abs() + e2);
    let a = [[-c.a1, -c.a2], [1.0, 0.0]];
    let (mut power, mut drift, mut powers) = (a, 0.0, 0.0);
    for m in 1..=1u32 << 20 {
        let most = norm(power) + drift;
        powers += most;
        if most <= 0.5 {
            let total = sum + err + 2.0 * powers * held;
            return Some(total * (1.0 + gamma(f64::from(m) + DIRECT as f64)));
        }
        drift = drift * norm(a) + gamma(2.0) * norm(power) * norm(a);
        power = times(power, a);
    }
    None
}
