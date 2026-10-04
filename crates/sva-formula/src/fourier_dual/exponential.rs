// Concern: duals the polynomial, exponential and delta families | Non-concern: their indicator-bounded forms (indicator.rs) | IO: (&SpectralAtom) -> Vec<SpectralAtom>

use std::f64::consts::TAU;

use crate::complex::C64;
use crate::spectral_sum::atom::{Exp, Factors, Poly, Singular, SpectralAtom};

/// `delta^(k)(u-t0)` duals to `(2pi i theta)^k e^{-2pi i theta t0}`.
pub(crate) fn delta_row(a: &SpectralAtom) -> Vec<SpectralAtom> {
    let Singular::Delta { at, order } = a.sing else {
        return Vec::new();
    };
    vec![SpectralAtom::new(
        a.c * C64::new(0.0, TAU).powi(u32::from(order)),
        Factors {
            poly: Poly::power(order),
            exp: Some(Exp::at(0.0, -TAU * at)),
            ..Factors::NONE
        },
        Singular::Regular,
        a.origin,
    )]
}

/// `u^n e^{iwu}` duals to `(i/2pi)^n delta^(n)(theta - w/2pi)`.
pub(crate) fn line_row(a: &SpectralAtom) -> Vec<SpectralAtom> {
    let omega = a.exp.map_or(0.0, |e| e.omega);
    vec![SpectralAtom::new(
        a.c * C64::new(0.0, 1.0 / TAU).powi(u32::from(a.poly.degree)),
        Factors::NONE,
        Singular::Delta {
            at: omega / TAU,
            order: a.poly.degree,
        },
        a.origin,
    )]
}
