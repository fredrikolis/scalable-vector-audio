// Concern: the energy a windowed atom's dual carries past the ceiling | Non-concern: which row that labels (collapse.rs) | IO: (&SpectralSum, ceiling) -> dB, or nothing where a window's dual is no sinc

use std::f64::consts::{PI, TAU};

use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};
use sva_formula::{C64, SpectralSum};

pub fn tail_db(n: &SpectralSum, ceiling: f64) -> Option<f64> {
    let mut held = 0.0f64;
    let mut gone = 0.0f64;
    for atom in n.atoms() {
        let Some(window) = atom.ind else { continue };
        if !sinc_dual(atom) {
            return None;
        }
        let width = window.r.value() - window.l.value();
        if !width.is_finite() || width <= 0.0 {
            continue;
        }
        let power = atom.c.norm_sqr();
        let hz = atom.exp.map_or(0.0, |e| e.omega / TAU);
        held += power * width;
        gone += power * (above(width, ceiling - hz) + above(width, ceiling + hz));
    }
    (held > 0.0).then(|| 10.0 * (gone / held).log10())
}

fn sinc_dual(a: &SpectralAtom) -> bool {
    a.poly == 0
        && a.gauss.is_none()
        && a.pole.is_none()
        && matches!(a.sing, Singular::Regular)
        && a.exp.is_none_or(|e| e.sigma == 0.0)
}

/// `(W sinc(W d))^2` from `x` to infinity, through the sine integral of Numerical Recipes
/// 6.9. Even in `x`, and `width` over the whole line.
fn above(width: f64, x: f64) -> f64 {
    if x < 0.0 {
        return width - above(width, -x);
    }
    if x == 0.0 {
        return width / 2.0;
    }
    let turns = TAU * width * x;
    (1.0 - turns.cos()) / (2.0 * PI * PI * x) + width / PI * (PI / 2.0 - sine_integral(turns))
}

fn sine_integral(x: f64) -> f64 {
    if x < 2.0 {
        return series(x);
    }
    let mut b = C64::new(1.0, x);
    let mut c = C64::real(1.0 / f64::MIN_POSITIVE);
    let mut d = b.inv();
    let mut h = d;
    for k in 2..MAX_STEPS {
        let a = -(((k - 1) * (k - 1)) as f64);
        b = b + C64::real(2.0);
        d = (d.scale(a) + b).inv();
        c = b + c.inv().scale(a);
        let step = c * d;
        h = h * step;
        if (step.re - 1.0).abs() + step.im.abs() < f64::EPSILON {
            break;
        }
    }
    PI / 2.0 + (C64::new(x.cos(), -x.sin()) * h).im
}

fn series(x: f64) -> f64 {
    let mut term = x;
    let mut sum = x;
    for k in 1..MAX_STEPS {
        let odd = (2 * k + 1) as f64;
        term *= -x * x / ((2 * k) as f64 * odd);
        sum += term / odd;
        if term.abs() < sum.abs() * f64::EPSILON {
            break;
        }
    }
    sum
}

const MAX_STEPS: usize = 200;
