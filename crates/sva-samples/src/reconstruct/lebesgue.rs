// Concern: bounds how far a kernel reading amplifies an error in the samples it takes | Non-concern: its error on a source component (bound.rs) | IO: (N, window table) -> f64

use std::f64::consts::PI;

/// `sup_f sum_j |sinc(j - f) w(j - f)|`, each of `oversample` pieces of `f` bounded by the
/// sup of every factor: `sin(pi f)` at the point nearest a half, `1/d` and the falling window
/// at the least distance, and the two nearest taps by `sinc` falling on `[0, 1]`.
pub(super) fn of(half_width: usize, oversample: usize, window: &[f64], table: f64) -> f64 {
    let (n, m) = (half_width as i64, oversample as i64);
    let w = |k: i64| window[(k + 1) as usize];
    let sinc = |x: f64| match x == 0.0 {
        true => 1.0,
        false => (PI * x).sin() / (PI * x),
    };
    let mut most = 1.0f64;
    for piece in 0..m {
        let (a, b) = (piece as f64 / m as f64, (piece + 1) as f64 / m as f64);
        let top = (PI * 0.5f64.clamp(a, b)).sin();
        let sum: f64 = ((1 - n)..=n)
            .map(|j| {
                let near = match j >= 1 {
                    true => (j - 1) * m + (m - piece - 1),
                    false => -j * m + piece,
                };
                let d = near as f64 / m as f64;
                let lobe = match j {
                    0 | 1 => sinc(d),
                    _ => top / (PI * d),
                };
                lobe * w(near)
            })
            .sum();
        most = most.max(sum);
    }
    most + table + super::bound::ROUNDING
}
