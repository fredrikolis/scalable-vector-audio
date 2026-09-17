// Concern: duals a pole atom into one-sided exponentials by residues, and a principal value into sgn | Non-concern: partial fractions (rational.rs) | IO: (&SpectralAtom) -> Vec<SpectralAtom> or Left

use std::f64::consts::{PI, TAU};

use crate::complex::C64;
use crate::origin::Origin;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Singular, SpectralAtom};
use crate::table::exponential::line_row;
use crate::table::indicator::signum;

/// The polynomial is first rewritten in powers of `u-p`, leaving pure poles beside one.
pub(crate) fn row(a: &SpectralAtom) -> Result<Vec<SpectralAtom>, Left> {
    let pole = a.pole.expect("the caller dispatched on a pole");
    let f0 = a.exp.map_or(0.0, |e| e.omega) / TAU;
    let m = pole.order;
    let mut out = Vec::new();
    let mut binomial = 1.0f64;
    for j in 0..=a.poly {
        let weight = a.c * pole.at.powi(u32::from(a.poly - j)).scale(binomial);
        if j < m {
            out.extend(residue_row(weight, pole.at, m - j, pole.pv, f0, a.origin)?);
        } else {
            out.extend(polynomial_row(weight, pole.at, j - m, f0, a.origin));
        }
        binomial = binomial * f64::from(a.poly - j) / f64::from(j + 1);
    }
    Ok(out)
}

/// A real pole reads as a principal value: the prescription that cancels a window's pair.
fn residue_row(
    w: C64,
    p: C64,
    q: u16,
    pv: bool,
    f0: f64,
    origin: Origin,
) -> Result<Vec<SpectralAtom>, Left> {
    if p.im == 0.0 || pv {
        if q > 1 {
            return Err(Left::new(
                origin,
                AtomSketch::of(Factor::PrincipalValue),
                LeftReason::PoleOrder(q),
            ));
        }
        let carried = C64::new(0.0, TAU * f0 * p.re).exp();
        return Ok(signum(
            w * C64::new(0.0, -PI) * carried,
            f0,
            Factors {
                exp: Some(Exp::at(0.0, -TAU * p.re)),
                ..Factors::NONE
            },
            origin,
        ));
    }

    let causal = p.im < 0.0;
    let lead = C64::new(0.0, if causal { -TAU } else { TAU });
    let factorial: f64 = (1..q).map(f64::from).product();
    let window = crate::spectral_sum::atom::Indicator {
        l: if causal {
            crate::closed_form::Edge::at(f0)
        } else {
            crate::closed_form::Edge::NegInf
        },
        r: if causal {
            crate::closed_form::Edge::PosInf
        } else {
            crate::closed_form::Edge::at(f0)
        },
    };
    let alpha = -C64::new(0.0, TAU) * p;
    // The growing half of the normalization stays the exponential's own reference.
    let carried = C64::new(0.0, -alpha.im * f0).exp();
    let base = w * lead * carried / C64::real(factorial);

    // (-2*pi*i*(theta - f0))^(q-1), expanded in powers of theta.
    let mut out = Vec::new();
    let mut binomial = 1.0f64;
    let degree = q - 1;
    for i in 0..=degree {
        let c = base
            * C64::new(0.0, -TAU).powi(u32::from(degree))
            * C64::real(binomial * (-f0).powi(i32::from(degree - i)));
        if !c.is_zero() {
            out.push(SpectralAtom::new(
                c,
                Factors {
                    poly: i,
                    exp: Some(Exp {
                        sigma: alpha.re,
                        omega: alpha.im,
                        mu: f0,
                    }),
                    ind: Some(window),
                    ..Factors::NONE
                },
                Singular::Regular,
                origin,
            ));
        }
        binomial = binomial * f64::from(degree - i) / f64::from(i + 1);
    }
    Ok(out)
}

fn polynomial_row(w: C64, p: C64, d: u16, f0: f64, origin: Origin) -> Vec<SpectralAtom> {
    let mut out = Vec::new();
    let mut binomial = 1.0f64;
    for i in (0..=d).rev() {
        let c = w * (-p).powi(u32::from(d - i)).scale(binomial);
        if !c.is_zero() {
            out.extend(line_row(&SpectralAtom::new(
                c,
                Factors {
                    poly: i,
                    exp: Some(Exp::at(0.0, TAU * f0)),
                    ..Factors::NONE
                },
                Singular::Regular,
                origin,
            )));
        }
        binomial = binomial * f64::from(i) / f64::from(d - i + 1);
    }
    out
}
