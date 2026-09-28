// Concern: bounds how far the table strays from the kernel | Non-concern: aliasing (tails.rs) | IO: (spec, beta) -> f64

use std::f64::consts::PI;

use super::{KernelSpec, i0};

const FACT: [f64; 5] = [1.0, 1.0, 2.0, 6.0, 24.0];

fn choose(i: usize, k: usize) -> f64 {
    FACT[i] / (FACT[k] * FACT[i - k])
}

/// `|sinc^(i)| <= pi^i/(i+1)`, or its decay `d` from zero.
fn sinc(i: usize, d: f64) -> f64 {
    let flat = PI.powi(i as i32) / (i + 1) as f64;
    if d <= 0.0 {
        return flat;
    }
    let decay: f64 = (0..=i)
        .map(|k| choose(i, k) * PI.powi((i - k) as i32) * FACT[k] / d.powi(k as i32))
        .sum();
    flat.min(decay / (PI * d))
}

/// Lagrange's remainder `3/128 h^4 |K''''|`; the window by Cauchy.
pub(super) fn error(spec: KernelSpec, beta: f64) -> f64 {
    let (n, h, r) = (spec.half_width as f64, 1.0 / spec.oversample as f64, 3.0);
    let window = i0(beta * (1.0 + (2.0 * n * r + r * r) / (n * n)).sqrt()) / i0(beta);
    let windowed = |m: usize| FACT[m] * window / r.powi(m as i32);
    let half = spec.half_width as i64;
    let per_tap: f64 = ((1 - half)..=half)
        .map(|j| {
            let d = match j == 0 || j == 1 {
                true => 0.0,
                false => ((j - 1).abs().min(j.abs()) as f64 - 2.0 * h).max(0.0),
            };
            (0..=4)
                .map(|i| choose(4, i) * sinc(i, d) * windowed(4 - i))
                .sum::<f64>()
        })
        .sum();
    3.0 / 128.0 * h.powi(4) * per_tap
}
