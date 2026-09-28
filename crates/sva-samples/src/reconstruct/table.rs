// Concern: bounds how far the tabulated window moves a reading | Non-concern: aliasing (tails.rs) | IO: (N, oversample, beta) -> f64

use std::f64::consts::PI;

use super::i0;

/// Any radius holds for an entire window; the least bound of these is kept.
const RADII: [f64; 6] = [2.0, 4.0, 8.0, 16.0, 32.0, 64.0];

/// Lagrange's remainder `3/128 h^4 |w''''|`, the window's by Cauchy on a disc about the
/// table's reach, under every tap's `|sinc|`: at most one for the two nearest, `1/(pi d)` else.
pub(super) fn error(half_width: usize, oversample: usize, beta: f64) -> f64 {
    let (n, h) = (half_width as f64, 1.0 / oversample as f64);
    let fourth = RADII
        .iter()
        .map(|r| {
            let held = i0(beta * (1.0 + (2.0 * (n + 1.0) * r + r * r) / (n * n)).sqrt()) / i0(beta);
            24.0 * held / r.powi(4)
        })
        .fold(f64::INFINITY, f64::min);
    3.0 / 128.0 * h.powi(4) * fourth * sincs(half_width)
}

/// `sum_j |sinc(j - f)|` over `2N` taps, any `f` in `(0, 1)`.
pub(super) fn sincs(half_width: usize) -> f64 {
    2.0 + 2.0 / PI * (1.0 + (half_width as f64).ln())
}
