// Concern: bounds how far a kernel reading amplifies an error in the samples it takes | Non-concern: its error on a source component (bound.rs) | IO: (spec, table) -> f64

use super::KernelSpec;

const PIECES: usize = 16;

/// `sup sum_j |w_j|` of `Kernel::weights`: each weight is a cubic in `t` per table cell,
/// bounded on a piece by its exact Taylor sum about the middle.
pub(super) fn of(spec: KernelSpec, table: &[f64], center: i64) -> f64 {
    let (n, step) = (spec.half_width as i64, spec.oversample as i64);
    let r = 0.5 / PIECES as f64;
    let mut most = 1.0f64;
    for at in -step..0 {
        let cubics: Vec<[f64; 4]> = ((1 - n)..=n)
            .map(|j| {
                let base = (center + j * step + at - 1) as usize;
                let c = &table[base..base + 4];
                [
                    c[1],
                    -c[0] / 3.0 - c[1] / 2.0 + c[2] - c[3] / 6.0,
                    c[0] / 2.0 - c[1] + c[2] / 2.0,
                    (c[3] - c[0]) / 6.0 + (c[1] - c[2]) / 2.0,
                ]
            })
            .collect();
        for piece in 0..PIECES {
            let m = (2 * piece + 1) as f64 * r;
            let sum: f64 = cubics
                .iter()
                .map(|[a0, a1, a2, a3]| {
                    let value = a0 + m * (a1 + m * (a2 + m * a3));
                    let slope = a1 + m * (2.0 * a2 + 3.0 * m * a3);
                    let curve = 2.0 * a2 + 6.0 * m * a3;
                    value.abs() + r * (slope.abs() + r * (curve.abs() / 2.0 + r * a3.abs()))
                })
                .sum();
            most = most.max(sum);
        }
    }
    most + super::bound::ROUNDING
}
