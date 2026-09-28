// Concern: the band-limited value of a lattice between its samples | Non-concern: the bound it carries (bound.rs), where positions come from | IO: (samples, fraction) -> f64

mod bound;
mod table;
mod tails;

use std::f64::consts::PI;
use std::sync::OnceLock;

pub use bound::Bound;

use crate::profile::{PSYCHOACOUSTIC_V1, Profile};

/// A Kaiser-windowed sinc, tabulated `oversample` points per sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KernelSpec {
    pub name: &'static str,
    pub half_width: usize,
    pub oversample: usize,
}

pub struct Kernel {
    spec: KernelSpec,
    beta: f64,
    table: Vec<f64>,
    center: i64,
}

pub fn kernel() -> &'static Kernel {
    static HELD: OnceLock<Kernel> = OnceLock::new();
    HELD.get_or_init(|| Kernel::of(&PSYCHOACOUSTIC_V1))
}

/// `I0(2 sqrt v)`, entire in `v`, so the window reads past its edge.
pub(crate) fn i0_series(v: f64) -> f64 {
    let (mut term, mut sum) = (1.0f64, 1.0f64);
    for k in 1..48 {
        term *= v / (k * k) as f64;
        sum += term;
    }
    sum
}

pub(crate) fn i0(x: f64) -> f64 {
    i0_series(x * x / 4.0)
}

impl Kernel {
    /// The main lobe ends where the band edge folds back.
    pub fn of(profile: &Profile) -> Kernel {
        let spec = profile.kernel;
        let n = spec.half_width as f64;
        let beta = 2.0 * PI * n * profile.fold_margin();
        let m = spec.oversample as i64;
        let reach = spec.half_width as i64 * m + 3;
        let norm = i0(beta);
        let table = (-reach..=reach)
            .map(|i| match (i.rem_euclid(m), i.div_euclid(m)) {
                (0, 0) => 1.0,
                (0, _) => 0.0,
                (r, whole) => {
                    let x = i as f64 / m as f64;
                    let sign = if whole % 2 == 0 { 1.0 } else { -1.0 };
                    let sinc = sign * (PI * r as f64 / m as f64).sin() / (PI * x);
                    sinc * i0_series(beta * beta * (1.0 - x * x / (n * n)) / 4.0) / norm
                }
            })
            .collect();
        Kernel {
            spec,
            beta,
            table,
            center: reach,
        }
    }

    pub fn half_width(&self) -> usize {
        self.spec.half_width
    }

    pub fn spec(&self) -> KernelSpec {
        self.spec
    }

    /// Tap `j` in `-N+1..=N` weighs sample `floor(p) + j`; `frac` is in `(0, 1)`.
    pub fn weights(&self, frac: f64, out: &mut Vec<f64>) {
        let g = -frac * self.spec.oversample as f64;
        let at = g.floor();
        let t = g - at;
        let lagrange = [
            -t * (t - 1.0) * (t - 2.0) / 6.0,
            (t + 1.0) * (t - 1.0) * (t - 2.0) / 2.0,
            -(t + 1.0) * t * (t - 2.0) / 2.0,
            (t + 1.0) * t * (t - 1.0) / 6.0,
        ];
        let (n, step) = (self.spec.half_width as i64, self.spec.oversample as i64);
        out.clear();
        for j in (1 - n)..=n {
            let base = (self.center + j * step + at as i64 - 1) as usize;
            let cell = &self.table[base..base + 4];
            out.push(
                lagrange[0] * cell[0]
                    + lagrange[1] * cell[1]
                    + lagrange[2] * cell[2]
                    + lagrange[3] * cell[3],
            );
        }
    }

    /// `None` where the band edge folds back inside the main lobe.
    pub fn bound(&self, band_hz: f64, source_rate: u32) -> Option<Bound> {
        bound::of(self.spec, self.beta, band_hz / f64::from(source_rate))
            .map(|b| Bound { band_hz, ..b })
    }
}

pub fn taps(floor: i64, half_width: usize) -> std::ops::RangeInclusive<i64> {
    let n = half_width as i64;
    (floor + 1 - n)..=(floor + n)
}
