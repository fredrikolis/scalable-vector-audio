// Concern: multiplies two atoms into one | Non-concern: summing like atoms (merge.rs) | IO: (SpectralAtom, SpectralAtom) -> Vec<SpectralAtom>

use crate::closed_form::Edge;
use crate::complex::C64;
use crate::rational::partial_fractions;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Gauss, Pole, Singular, SpectralAtom};

/// Empty where the two indicators do not meet; several where two distinct poles partial-
/// fraction, or where a delta derivative expands by Leibniz.
pub fn times(a: &SpectralAtom, b: &SpectralAtom) -> Result<Vec<SpectralAtom>, Left> {
    match (a.sing, b.sing) {
        (Singular::Regular, Singular::Regular) => smooth_times(a, b),
        (Singular::Delta { at, order }, Singular::Regular) => leibniz(a, b, at, order),
        (Singular::Regular, Singular::Delta { at, order }) => leibniz(b, a, at, order),
        _ => Err(Left::new(
            a.origin,
            AtomSketch::pair(Factor::Delta, Factor::Delta),
            LeftReason::Nonlinearity,
        )),
    }
}

/// Two growing halves meet at one reference, the mean their rates weight, so the product
/// carries no prefactor at all; only a growth that cancels leaves one behind.
fn combine(x: Exp, y: Exp) -> Option<(Exp, C64)> {
    let sigma = x.sigma + y.sigma;
    let omega = x.omega + y.omega;
    if sigma == 0.0 {
        let carried = C64::real(-x.sigma * x.mu - y.sigma * y.mu).exp();
        return carried
            .is_finite()
            .then_some((Exp::at(0.0, omega), carried));
    }
    let mu = (x.sigma * x.mu + y.sigma * y.mu) / sigma;
    Some((Exp { sigma, omega, mu }, C64::ONE))
}

fn smooth_times(a: &SpectralAtom, b: &SpectralAtom) -> Result<Vec<SpectralAtom>, Left> {
    let ind = match (a.ind, b.ind) {
        (Some(x), Some(y)) => {
            let met = x.meet(y);
            if met.is_empty() {
                return Ok(Vec::new());
            }
            Some(met)
        }
        (x, y) => x.or(y),
    };

    let mut c = a.c * b.c;
    let exp = match (a.exp, b.exp) {
        (Some(x), Some(y)) => {
            let (held, carried) = combine(x, y).ok_or_else(|| {
                Left::new(
                    a.origin,
                    AtomSketch::pair(Factor::Exponential, Factor::Exponential),
                    LeftReason::NotTempered,
                )
            })?;
            c = c * carried;
            Some(held)
        }
        (x, y) => x.or(y),
    };
    let gauss = match (a.gauss, b.gauss) {
        (Some(x), Some(y)) => {
            let sum = x.a + y.a;
            let gap = x.mu - y.mu;
            c = c.scale((-x.a * y.a * gap * gap / sum).exp());
            Some(Gauss {
                a: sum,
                mu: (x.a * x.mu + y.a * y.mu) / sum,
            })
        }
        (x, y) => x.or(y),
    };

    let base = Factors {
        poly: a.poly + b.poly,
        exp,
        gauss,
        ind,
        pole: None,
    };
    match (a.pole, b.pole) {
        (None, None) => Ok(vec![SpectralAtom::new(
            c,
            base,
            Singular::Regular,
            a.origin,
        )]),
        (Some(p), None) | (None, Some(p)) => Ok(vec![SpectralAtom::new(
            c,
            Factors {
                pole: Some(p),
                ..base
            },
            Singular::Regular,
            a.origin,
        )]),
        (Some(p), Some(q)) if p.at == q.at => Ok(vec![SpectralAtom::new(
            c,
            Factors {
                pole: Some(Pole {
                    at: p.at,
                    order: p.order + q.order,
                    pv: p.pv || q.pv,
                }),
                ..base
            },
            Singular::Regular,
            a.origin,
        )]),
        (Some(p), Some(q)) => split_poles(c, base, p, q, a),
    }
}

/// Two distinct poles are one rational, and a rational in A is its residues. The principal
/// value stays with the pole it was written on: only that one has no ordinary value.
fn split_poles(
    c: C64,
    base: Factors,
    p: Pole,
    q: Pole,
    at: &SpectralAtom,
) -> Result<Vec<SpectralAtom>, Left> {
    let mut roots = vec![p.at; usize::from(p.order)];
    roots.extend(std::iter::repeat_n(q.at, usize::from(q.order)));
    let (_, residues) = partial_fractions(&[C64::ONE], &roots).map_err(|reason| {
        Left::new(
            at.origin,
            AtomSketch::pair(Factor::Pole, Factor::Pole),
            reason,
        )
    })?;
    Ok(residues
        .into_iter()
        .filter(|r| !r.weight.is_zero())
        .map(|r| {
            SpectralAtom::new(
                c * r.weight,
                Factors {
                    pole: Some(Pole {
                        at: r.at,
                        order: r.order,
                        pv: if r.at == p.at { p.pv } else { q.pv },
                    }),
                    ..base
                },
                Singular::Regular,
                at.origin,
            )
        })
        .collect())
}

/// `g(x) * delta^(k)(x - a)` is the alternating binomial sum over `g`'s derivatives at `a`.
fn leibniz(
    delta: &SpectralAtom,
    smooth: &SpectralAtom,
    at: f64,
    order: u16,
) -> Result<Vec<SpectralAtom>, Left> {
    if let Some(i) = smooth.ind
        && (i.l == Edge::At(crate::complex::canonical(at))
            || i.r == Edge::At(crate::complex::canonical(at)))
    {
        return Err(Left::new(
            delta.origin,
            AtomSketch::pair(Factor::Delta, Factor::Indicator),
            LeftReason::Nonlinearity,
        ));
    }
    if smooth.ind.is_some_and(|i| !i.contains(at)) {
        return Ok(Vec::new());
    }
    let mut level = vec![SpectralAtom::new(
        smooth.c,
        Factors {
            ind: None,
            ..smooth.factors()
        },
        Singular::Regular,
        smooth.origin,
    )];

    let mut out = Vec::new();
    let mut binomial = 1i64;
    for j in 0..=order {
        let mut value = C64::ZERO;
        for atom in &level {
            let Some(v) = atom.smooth_at(at) else {
                return Err(Left::new(
                    delta.origin,
                    AtomSketch::pair(Factor::Delta, Factor::Pole),
                    LeftReason::Nonlinearity,
                ));
            };
            value = value + v;
        }
        let sign = if j % 2 == 0 { 1.0 } else { -1.0 };
        out.push(SpectralAtom::new(
            delta.c * value.scale(sign * binomial as f64),
            Factors::NONE,
            Singular::Delta {
                at,
                order: order - j,
            },
            delta.origin,
        ));
        binomial = binomial * i64::from(order - j) / i64::from(j + 1);
        level = level.iter().flat_map(SpectralAtom::derivative).collect();
    }
    Ok(out)
}
