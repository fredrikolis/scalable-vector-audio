// Concern: differentiates, Hilbert-transforms and envelopes a SpectralSum | Non-concern: the transform itself (table/) | IO: (&SpectralSum) -> SpectralSum or Left

use crate::closed_form::{Body, Edge, Part};
use crate::complex::C64;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Gauss, Indicator, Pole, Singular, SpectralAtom};
use crate::spectral_sum::merge::simplify;
use crate::spectral_sum::product::times;
use crate::spectral_sum::{Lane, SpectralSum};
use crate::table::{dual, reflect};

/// The square of the analytic signal's modulus, which is in A; the root is the observation's
/// job, so A stays closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub squared: SpectralSum,
}

/// Total: A is closed under `d/dx`. A series differentiates termwise, lazily where its term
/// is not one of the shapes the table reads.
pub fn d_dt(n: &SpectralSum) -> SpectralSum {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in &n.lanes {
        let mut out = Lane {
            atoms: lane
                .atoms
                .iter()
                .flat_map(SpectralAtom::derivative)
                .collect(),
            series: lane.series.iter().map(derive_series).collect(),
            modal: Vec::new(),
        };
        for bank in &lane.modal {
            let expanded = crate::modal::atoms(bank, crate::origin::Origin::UNKNOWN);
            out.atoms
                .extend(expanded.iter().flat_map(SpectralAtom::derivative));
        }
        simplify(&mut out);
        lanes.push(out);
    }
    SpectralSum::of(n.var, lanes)
}

fn derive_series(s: &crate::closed_form::Series) -> crate::closed_form::Series {
    crate::closed_form::Series {
        term: Part::new(
            s.term.origin,
            Body::Deriv {
                order: 1,
                of: s.term.clone(),
            },
        ),
        ..s.clone()
    }
}

/// Dual, multiply by `-i*sgn`, dual, reflect. It is never a convolution with `pv(t)/pi`.
pub fn hilbert(n: &SpectralSum) -> Result<SpectralSum, Left> {
    let spectrum = dual(n)?;
    let mut lanes = Vec::with_capacity(spectrum.lanes.len());
    for lane in &spectrum.lanes {
        if !lane.is_finite_sum() {
            return Err(Left::new(
                crate::origin::Origin::UNKNOWN,
                AtomSketch::of(Factor::Value),
                LeftReason::SeriesNonUniform,
            ));
        }
        let mut atoms = Vec::new();
        for a in &lane.atoms {
            atoms.extend(times_signum(a)?);
        }
        let mut out = Lane::of(atoms);
        simplify(&mut out);
        lanes.push(out);
    }
    reflect(&dual(&SpectralSum::of(spectrum.var, lanes))?)
}

/// `sgn` is smooth away from the origin, so it reads a delta's own sign; a delta AT the
/// origin is `H{1} = 0`, and its derivatives are the duals of the polynomials.
fn times_signum(a: &SpectralAtom) -> Result<Vec<SpectralAtom>, Left> {
    let minus_i = C64::new(0.0, -1.0);
    if let Singular::Delta { at, order } = a.sing {
        if at == 0.0 {
            return match order {
                0 => Ok(Vec::new()),
                _ => Err(Left::new(
                    a.origin,
                    AtomSketch::of(Factor::Delta),
                    LeftReason::HilbertOfPolynomial,
                )),
            };
        }
        let sign = if at > 0.0 { 1.0 } else { -1.0 };
        return Ok(vec![SpectralAtom::new(
            a.c * minus_i.scale(sign),
            Factors::NONE,
            a.sing,
            a.origin,
        )]);
    }
    if a.pole.is_some() {
        return Err(Left::new(
            a.origin,
            AtomSketch::pair(
                if a.pole.is_some_and(|p| p.pv) {
                    Factor::PrincipalValue
                } else {
                    Factor::Pole
                },
                Factor::Indicator,
            ),
            LeftReason::PoleTimesIndicator,
        ));
    }
    if a.gauss.is_some() {
        return Err(Left::new(
            a.origin,
            AtomSketch::pair(Factor::Gaussian, Factor::Indicator),
            LeftReason::GaussianTimesIndicator,
        ));
    }
    let halves = [
        (1.0, Edge::at(0.0), Edge::PosInf),
        (-1.0, Edge::NegInf, Edge::at(0.0)),
    ];
    Ok(halves
        .into_iter()
        .filter_map(|(sign, l, r)| {
            let window = a
                .ind
                .map_or(Indicator { l, r }, |w| w.meet(Indicator { l, r }));
            (!window.is_empty()).then(|| {
                SpectralAtom::new(
                    a.c * minus_i.scale(sign),
                    Factors {
                        ind: Some(window),
                        ..a.factors()
                    },
                    Singular::Regular,
                    a.origin,
                )
            })
        })
        .collect())
}

pub fn analytic(n: &SpectralSum) -> Result<SpectralSum, Left> {
    let quadrature = hilbert(n)?;
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for (real, imaginary) in n.lanes.iter().zip(quadrature.lanes.iter()) {
        let mut out = Lane::of(
            real.atoms
                .iter()
                .copied()
                .chain(
                    imaginary
                        .atoms
                        .iter()
                        .map(|a| a.with(a.c * C64::I, a.factors())),
                )
                .collect(),
        );
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

pub fn envelope(n: &SpectralSum) -> Result<Envelope, Left> {
    let signal = analytic(n)?;
    let mut lanes = Vec::with_capacity(signal.lanes.len());
    for lane in &signal.lanes {
        let mut atoms = Vec::new();
        for a in &lane.atoms {
            for b in &lane.atoms {
                atoms.extend(times(a, &conjugate(b))?);
            }
        }
        let mut out = Lane::of(atoms);
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(Envelope {
        squared: SpectralSum::of(signal.var, lanes),
    })
}

fn conjugate(a: &SpectralAtom) -> SpectralAtom {
    if a.is_delta() {
        return SpectralAtom::new(a.c.conj(), Factors::NONE, a.sing, a.origin);
    }
    let f = a.factors();
    SpectralAtom::new(
        a.c.conj(),
        Factors {
            exp: f.exp.map(|e| Exp {
                omega: -e.omega,
                ..e
            }),
            gauss: f.gauss.map(|g| Gauss { ..g }),
            pole: f.pole.map(|p| Pole {
                at: p.at.conj(),
                ..p
            }),
            ..f
        },
        Singular::Regular,
        a.origin,
    )
}
