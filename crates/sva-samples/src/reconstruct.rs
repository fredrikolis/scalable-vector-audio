// Concern: the band-limited value of a lattice between its samples, per length of the family | Non-concern: the bound it carries (bound.rs), where positions come from | IO: (samples, fraction) -> f64

mod bound;
mod lebesgue;
mod table;
mod tails;

use std::f64::consts::PI;
use std::sync::OnceLock;

pub use bound::Bound;

use crate::profile::{PSYCHOACOUSTIC_V1, Profile};

/// Kaiser-windowed sincs `step`, `2 step`, ... `most` samples either side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KernelFamily {
    pub name: &'static str,
    pub step: usize,
    pub most: usize,
    pub oversample: usize,
}

impl KernelFamily {
    pub fn lengths(self, most: usize) -> impl Iterator<Item = usize> {
        (1..=self.most.min(most) / self.step).map(move |k| k * self.step)
    }
}

pub struct Kernel {
    name: &'static str,
    half_width: usize,
    oversample: usize,
    beta: f64,
    lobe: f64,
    /// At `(k - 1) / oversample`.
    window: Vec<f64>,
    lebesgue: f64,
}

const MEMBERS: usize = PSYCHOACOUSTIC_V1.kernel.most / PSYCHOACOUSTIC_V1.kernel.step;

pub fn kernel(half_width: usize) -> &'static Kernel {
    static HELD: [OnceLock<Kernel>; MEMBERS] = [const { OnceLock::new() }; MEMBERS];
    let family = PSYCHOACOUSTIC_V1.kernel;
    assert!(
        half_width.is_multiple_of(family.step) && (family.step..=family.most).contains(&half_width),
        "{half_width} is no length of the {} family",
        family.name
    );
    HELD[half_width / family.step - 1].get_or_init(|| Kernel::of(&PSYCHOACOUSTIC_V1, half_width))
}

pub fn shortest(most: usize, meets: impl Fn(&Bound) -> bool) -> Option<&'static Kernel> {
    let p = PSYCHOACOUSTIC_V1;
    p.kernel.lengths(most).map(kernel).find(|k| {
        k.bound(p.ceiling_hz, p.lattice_hz)
            .is_some_and(|b| meets(&b))
    })
}

/// For a reading nothing feeds back.
pub fn plain() -> &'static Kernel {
    static HELD: OnceLock<usize> = OnceLock::new();
    let p = PSYCHOACOUSTIC_V1;
    kernel(*HELD.get_or_init(|| {
        shortest(p.kernel.most, |b| b.in_band <= p.half_lsb())
            .expect("the profile's family reaches its own precision")
            .half_width
    }))
}

/// `I0(2 sqrt v)`, entire in `v`, so the window reads past its edge; terms peak near `sqrt |v|`.
pub(crate) fn i0_series(v: f64) -> f64 {
    let (mut term, mut sum) = (1.0f64, 1.0f64);
    let least = v.abs().sqrt().ceil() as usize + 2;
    for k in 1..4096 {
        term *= v / (k * k) as f64;
        sum += term;
        if k > least && term.abs() <= sum.abs() * f64::EPSILON / 4.0 {
            break;
        }
    }
    sum
}

pub(crate) fn i0(x: f64) -> f64 {
    i0_series(x * x / 4.0)
}

impl Kernel {
    /// The main lobe ends where the band edge folds back.
    pub fn of(profile: &Profile, half_width: usize) -> Kernel {
        let family = profile.kernel;
        let lobe = profile.fold_margin();
        let n = half_width as f64;
        let beta = 2.0 * PI * n * lobe;
        let m = family.oversample;
        let norm = i0(beta);
        let window: Vec<f64> = (0..=half_width * m + 3)
            .map(|k| {
                let x = (k as f64 - 1.0) / m as f64;
                i0_series(beta * beta * (1.0 - x * x / (n * n)) / 4.0) / norm
            })
            .collect();
        let lebesgue = lebesgue::of(half_width, m, &window, table::error(half_width, m, beta));
        Kernel {
            name: family.name,
            half_width,
            oversample: m,
            beta,
            lobe,
            window,
            lebesgue,
        }
    }

    pub fn half_width(&self) -> usize {
        self.half_width
    }

    fn window_at(&self, a: f64) -> f64 {
        let g = a * self.oversample as f64;
        let at = g.floor();
        let t = g - at;
        let cell = &self.window[at as usize..at as usize + 4];
        -t * (t - 1.0) * (t - 2.0) / 6.0 * cell[0]
            + (t + 1.0) * (t - 1.0) * (t - 2.0) / 2.0 * cell[1]
            - (t + 1.0) * t * (t - 2.0) / 2.0 * cell[2]
            + (t + 1.0) * t * (t - 1.0) / 6.0 * cell[3]
    }

    /// Tap `j` in `-N+1..=N` weighs sample `floor(p) + j`, `frac` in `(0, 1)`; the sinc is
    /// exact by `sin(pi (j - f)) = (-1)^(j+1) sin(pi f)`.
    pub fn weights(&self, frac: f64, out: &mut Vec<f64>) {
        let n = self.half_width as i64;
        let s = (PI * frac).sin() / PI;
        out.clear();
        for j in (1 - n)..=n {
            let x = j as f64 - frac;
            let sign = if j % 2 == 0 { -1.0 } else { 1.0 };
            out.push(sign * s / x * self.window_at(x.abs()));
        }
    }

    /// `None` where the band edge folds back inside the main lobe.
    pub fn bound(&self, band_hz: f64, source_rate: u32) -> Option<Bound> {
        bound::of(
            self.name,
            (self.half_width, self.oversample),
            (self.beta, self.lobe),
            band_hz / f64::from(source_rate),
            self.lebesgue,
        )
        .map(|b| Bound { band_hz, ..b })
    }
}

pub fn taps(floor: i64, half_width: usize) -> std::ops::RangeInclusive<i64> {
    let n = half_width as i64;
    (floor + 1 - n)..=(floor + n)
}
