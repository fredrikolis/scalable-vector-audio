// Concern: duals an indicator-bounded atom into pole and principal-value atoms | Non-concern: the unbounded forms (exponential.rs) | IO: (&SpectralAtom) -> Vec<SpectralAtom> or Left

use std::f64::consts::TAU;

use crate::closed_form::Edge;
use crate::complex::C64;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Indicator, Pole, Singular, SpectralAtom};

/// Two finite edges give a difference of pole families; one gives a half, or Heaviside.
pub(crate) fn row(a: &SpectralAtom) -> Result<Vec<SpectralAtom>, Left> {
    let window = a.ind.expect("the caller dispatched on an indicator");
    let alpha = a.exp.map_or(C64::ZERO, |e| C64::new(e.sigma, e.omega));
    let pole = (-C64::I * alpha).over(TAU);

    match (window.l, window.r) {
        (Edge::At(l), Edge::At(r)) => Ok([
            edge_terms(a, alpha, pole, f64::from_bits(l), C64::ONE),
            edge_terms(a, alpha, pole, f64::from_bits(r), -C64::ONE),
        ]
        .concat()),
        (Edge::At(l), Edge::PosInf) if alpha.re < 0.0 => {
            Ok(edge_terms(a, alpha, pole, f64::from_bits(l), C64::ONE))
        }
        (Edge::NegInf, Edge::At(r)) if alpha.re > 0.0 => {
            Ok(edge_terms(a, alpha, pole, f64::from_bits(r), -C64::ONE))
        }
        (Edge::At(l), Edge::PosInf) => heaviside(a, pole, f64::from_bits(l), C64::ONE),
        (Edge::NegInf, Edge::At(r)) => heaviside(a, pole, f64::from_bits(r), -C64::ONE),
        _ => Err(Left::new(
            a.origin,
            AtomSketch::of(Factor::Indicator),
            LeftReason::NotTempered,
        )),
    }
}

/// One end of `integral u^n e^{-s u} du`, `s = 2*pi*i*theta - alpha`.
fn edge_terms(a: &SpectralAtom, alpha: C64, pole: C64, edge: f64, sign: C64) -> Vec<SpectralAtom> {
    let n = a.poly;
    let mut out = Vec::with_capacity(usize::from(n) + 1);
    let mut falling = 1.0f64;
    for k in 0..=n {
        let power = i32::from(n - k);
        let weight = C64::real(falling * edge.powi(power));
        let scale = C64::new(0.0, TAU).powi(u32::from(k) + 1);
        let c = a.c * sign * weight * (alpha.scale(edge)).exp() / scale;
        if !c.is_zero() {
            out.push(SpectralAtom::new(
                c,
                Factors {
                    exp: Some(Exp::at(0.0, -TAU * edge)),
                    pole: Some(Pole {
                        at: pole,
                        order: k + 1,
                        pv: false,
                    }),
                    ..Factors::NONE
                },
                Singular::Regular,
                a.origin,
            ));
        }
        falling *= f64::from(n - k);
    }
    out
}

/// `1[l,inf)` at zero growth: half a delta beside a principal value.
fn heaviside(a: &SpectralAtom, pole: C64, edge: f64, sign: C64) -> Result<Vec<SpectralAtom>, Left> {
    if a.poly > 0 {
        return Err(Left::new(
            a.origin,
            AtomSketch::pair(Factor::Polynomial, Factor::Indicator),
            LeftReason::NotInTable,
        ));
    }
    let at = pole.re;
    let phase = C64::new(0.0, TAU * at * edge).exp();
    Ok(vec![
        SpectralAtom::new(
            a.c.scale(0.5),
            Factors::NONE,
            Singular::Delta { at, order: 0 },
            a.origin,
        ),
        SpectralAtom::new(
            a.c * sign * phase / C64::new(0.0, TAU),
            Factors {
                exp: Some(Exp::at(0.0, -TAU * edge)),
                pole: Some(Pole {
                    at: C64::real(at),
                    order: 1,
                    pv: true,
                }),
                ..Factors::NONE
            },
            Singular::Regular,
            a.origin,
        ),
    ])
}

pub(crate) fn signum(
    weight: C64,
    at: f64,
    factors: Factors,
    origin: crate::origin::Origin,
) -> Vec<SpectralAtom> {
    vec![
        SpectralAtom::new(
            weight,
            Factors {
                ind: Some(Indicator {
                    l: Edge::at(at),
                    r: Edge::PosInf,
                }),
                ..factors
            },
            Singular::Regular,
            origin,
        ),
        SpectralAtom::new(
            -weight,
            Factors {
                ind: Some(Indicator {
                    l: Edge::NegInf,
                    r: Edge::at(at),
                }),
                ..factors
            },
            Singular::Regular,
            origin,
        ),
    ]
}
