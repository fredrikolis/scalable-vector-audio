// Concern: differentiates, Hilbert-transforms and envelopes a SpectralSum, and bounds a slope | Non-concern: the transform (fourier_dual/) | IO: (&SpectralSum) -> SpectralSum or Left; (&Body) -> f64

use crate::closed_form::{Body, Edge, Part};
use crate::complex::C64;
use crate::fourier_dual::{dual_read, reflect_read};
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Gauss, Indicator, Pole, Singular, SpectralAtom};
use crate::spectral_sum::merge::simplify;
use crate::spectral_sum::product::times;
use crate::spectral_sum::{Lane, SpectralSum};
use crate::through::Reads;

/// The square of the analytic signal's modulus, which is in A; the root is the observation's
/// job, so A stays closed.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub squared: SpectralSum,
}

/// A is closed under `d/dx` up to an order or degree past `u16`, which refuses. A series
/// differentiates termwise, lazily where its term is not one of the shapes the Fourier dual rules read.
pub fn d_dt(n: &SpectralSum) -> Result<SpectralSum, Left> {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in &n.lanes {
        let mut out = Lane {
            atoms: Vec::new(),
            series: lane.series.iter().map(derive_series).collect(),
            modal: Vec::new(),
        };
        for atom in &lane.atoms {
            out.atoms.extend(atom.derivative()?);
        }
        for bank in &lane.modal {
            let expanded = crate::modal::atoms(bank, crate::origin::Origin::UNKNOWN);
            for atom in &expanded {
                out.atoms.extend(atom.derivative()?);
            }
        }
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

/// At least `sup |f'(t)|` over every instant: each atom's own sup, summed. `None` where an
/// atom's slope grows without bound either way in time, or `f` has no atom sum.
pub fn steepest(f: &Body) -> Option<f64> {
    steepest_read(f, &crate::through::Opaque)
}

/// The same, each ref read as `reads` answers it.
pub fn steepest_read(f: &Body, reads: &dyn crate::through::Reads) -> Option<f64> {
    let normalized =
        crate::spectral_sum::build::normalize_read(f, crate::closed_form::Var::T, reads);
    let slope = d_dt(&normalized.ok()?).ok()?;
    let mut held = 0.0;
    for lane in &slope.lanes {
        if !lane.series.is_empty() || !lane.modal.is_empty() {
            return None;
        }
        for atom in &lane.atoms {
            held += crate::spectral_sum::sup::magnitude_upper_bound_from_instant(
                atom,
                f64::NEG_INFINITY,
            )?;
        }
    }
    held.is_finite().then_some(held)
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
    hilbert_read(n, &crate::through::Opaque)
}

pub fn hilbert_read(n: &SpectralSum, reads: &dyn Reads) -> Result<SpectralSum, Left> {
    let spectrum = dual_read(n, reads)?;
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
    reflect_read(
        &dual_read(&SpectralSum::of(spectrum.var, lanes), reads)?,
        reads,
    )
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
    analytic_read(n, &crate::through::Opaque)
}

pub fn analytic_read(n: &SpectralSum, reads: &dyn Reads) -> Result<SpectralSum, Left> {
    let quadrature = hilbert_read(n, reads)?;
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
    envelope_read(n, &crate::through::Opaque)
}

pub fn envelope_read(n: &SpectralSum, reads: &dyn Reads) -> Result<Envelope, Left> {
    let signal = analytic_read(n, reads)?;
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
