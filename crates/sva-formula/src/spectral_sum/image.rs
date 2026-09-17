// Concern: the atoms each written constructor lowers to, and the window one lifts off | Non-concern: the lane algebra (build.rs) | IO: (a constructor's arguments) -> SpectralSum or a window

use crate::affine::{apply_scalar, completed_square, exact_affine};
use crate::closed_form::{Body, Edge, Fold, Part, Series, Unary, Var};
use crate::complex::C64;
use crate::origin::Origin;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Gauss, Indicator, Pole, Singular, SpectralAtom};
use crate::spectral_sum::merge::simplify;
use crate::spectral_sum::{Lane, SpectralSum};

pub fn left(origin: Origin, first: Factor, reason: LeftReason) -> Left {
    Left::new(origin, AtomSketch::of(first), reason)
}

/// A constant that is not finite is no atom: a division, a remainder or a logarithm at zero
/// names no number, and A holds none.
pub fn constant(var: Var, c: C64, origin: Origin) -> Result<SpectralSum, Left> {
    match c.is_finite() {
        true => Ok(one(var, SpectralAtom::constant(c, origin))),
        false => Err(left(origin, Factor::Amplitude, LeftReason::NoValue)),
    }
}

pub fn one(var: Var, atom: SpectralAtom) -> SpectralSum {
    let mut lane = Lane::of(vec![atom]);
    simplify(&mut lane);
    SpectralSum::of(var, vec![lane])
}

pub fn apply(op: Unary, arg: &Part, origin: Origin, var: Var) -> Result<SpectralSum, Left> {
    let affine = exact_affine(&arg.body);
    if let Some((a, b)) = affine
        && a.is_zero()
    {
        return constant(var, apply_scalar(op, b), origin);
    }
    if op == Unary::Exp
        && affine.is_none()
        && let Some(q) = completed_square(&arg.body)
    {
        return Ok(one(
            var,
            SpectralAtom::new(
                q.amplitude,
                Factors {
                    exp: Some(Exp::at(0.0, q.omega)),
                    gauss: Some(Gauss { a: q.a, mu: q.mu }),
                    ..Factors::NONE
                },
                Singular::Regular,
                origin,
            ),
        ));
    }
    let Some((a, b)) = affine.filter(|_| op.is_closed()) else {
        return Err(if op.is_closed() {
            left(
                arg.origin,
                Factor::Exponential,
                LeftReason::NonAffineArgument,
            )
        } else {
            left(origin, Factor::Value, LeftReason::Nonlinearity)
        });
    };
    let atoms = match op {
        Unary::Exp => vec![line_atom(b.exp(), a, origin)],
        Unary::Cos => vec![
            line_atom((C64::I * b).exp().scale(0.5), C64::I * a, origin),
            line_atom((-C64::I * b).exp().scale(0.5), -C64::I * a, origin),
        ],
        _ => vec![
            line_atom((C64::I * b).exp() * -C64::I.scale(0.5), C64::I * a, origin),
            line_atom((-C64::I * b).exp() * C64::I.scale(0.5), -C64::I * a, origin),
        ],
    };
    let mut lane = Lane::of(atoms);
    simplify(&mut lane);
    Ok(SpectralSum::of(var, vec![lane]))
}

fn line_atom(c: C64, alpha: C64, origin: Origin) -> SpectralAtom {
    SpectralAtom::new(
        c,
        Factors {
            exp: Some(Exp::at(alpha.re, alpha.im)),
            ..Factors::NONE
        },
        Singular::Regular,
        origin,
    )
}

pub fn fold(op: Fold, args: &[Part], origin: Origin, var: Var) -> Result<SpectralSum, Left> {
    let values: Option<Vec<C64>> = args
        .iter()
        .map(|p| {
            exact_affine(&p.body)
                .filter(|(a, _)| a.is_zero())
                .map(|x| x.1)
        })
        .collect();
    let Some(values) = values else {
        return Err(left(origin, Factor::Value, LeftReason::Nonlinearity));
    };
    let folded = match (op, values.as_slice()) {
        (Fold::Max, [a, b]) => C64::real(a.re.max(b.re)),
        (Fold::Min, [a, b]) => C64::real(a.re.min(b.re)),
        (Fold::Mod, [a, b]) => C64::real(a.re.rem_euclid(b.re)),
        _ => return Err(left(origin, Factor::Value, LeftReason::Nonlinearity)),
    };
    constant(var, folded, origin)
}

/// `delta^(k)(a*x + b)` places one delta at the argument's zero, scaled by `a^k * |a|`.
pub fn delta(at: &Part, order: u16, var: Var) -> Result<SpectralSum, Left> {
    let (a, b) = real_affine(at)?;
    let scale = a.powi(i32::from(order)) * a.abs();
    Ok(one(
        var,
        SpectralAtom::new(
            C64::real(1.0 / scale),
            Factors::NONE,
            Singular::Delta { at: -b / a, order },
            at.origin,
        ),
    ))
}

pub fn principal_value(at: &Part, var: Var) -> Result<SpectralSum, Left> {
    let (a, b) = real_affine(at)?;
    Ok(one(
        var,
        SpectralAtom::new(
            C64::real(1.0 / a),
            Factors {
                pole: Some(Pole {
                    at: C64::real(-b / a),
                    order: 1,
                    pv: true,
                }),
                ..Factors::NONE
            },
            Singular::Regular,
            at.origin,
        ),
    ))
}

fn real_affine(at: &Part) -> Result<(f64, f64), Left> {
    match exact_affine(&at.body) {
        Some((a, b)) if a.is_real() && b.is_real() && a.re != 0.0 => Ok((a.re, b.re)),
        _ => Err(left(
            at.origin,
            Factor::Delta,
            LeftReason::NonAffineArgument,
        )),
    }
}
/// The `a*x + b` an atom sum spells, when it spells one.
pub fn affine_atoms(n: &SpectralSum) -> Option<(C64, C64)> {
    let [lane] = n.lanes.as_slice() else {
        return None;
    };
    if !lane.is_finite_sum() {
        return None;
    }
    let (mut a, mut b) = (C64::ZERO, C64::ZERO);
    for atom in &lane.atoms {
        if atom.is_delta() || atom.exp.is_some() || atom.gauss.is_some() {
            return None;
        }
        if atom.ind.is_some() || atom.pole.is_some() {
            return None;
        }
        match atom.poly {
            0 => b = b + atom.c,
            1 => a = a + atom.c,
            _ => return None,
        }
    }
    (!a.is_zero()).then_some((a, b))
}

pub fn shift(n: SpectralSum, by: f64) -> Result<SpectralSum, Left> {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in n.lanes {
        let lane = lane.expanded();
        let mut out = Lane::of(lane.atoms.iter().flat_map(|a| shift_atom(a, by)).collect());
        simplify(&mut out);
        for s in &lane.series {
            out.series.push(shift_series(s, by)?);
        }
        lanes.push(out);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

/// A shift of a series is the shift of each of its terms: the free variable moves under the
/// sum the same way it moves under a finite one, so the pair and its term count both stand.
fn shift_series(s: &Series, by: f64) -> Result<Series, Left> {
    if crate::closed_form::shifts_opaquely(&s.term.body) {
        return Err(left(
            s.term.origin,
            Factor::Value,
            LeftReason::Unsubstituted,
        ));
    }
    Ok(Series {
        term: Part::new(
            s.term.origin,
            crate::closed_form::shift_line(&s.term.body, by),
        ),
        ..s.clone()
    })
}

/// `x -> x - by` in every factor; the polynomial is the only one that splits, binomially.
fn shift_atom(atom: &SpectralAtom, by: f64) -> Vec<SpectralAtom> {
    if let Singular::Delta { at, order } = atom.sing {
        return vec![SpectralAtom::new(
            atom.c,
            Factors::NONE,
            Singular::Delta { at: at + by, order },
            atom.origin,
        )];
    }
    let f = atom.factors();
    let moved = Factors {
        poly: 0,
        exp: f.exp.map(|e| Exp { mu: e.mu + by, ..e }),
        gauss: f.gauss.map(|g| Gauss { mu: g.mu + by, ..g }),
        ind: f.ind.map(|i| Indicator {
            l: Edge::at(i.l.value() + by),
            r: Edge::at(i.r.value() + by),
        }),
        pole: f.pole.map(|p| Pole {
            at: p.at + C64::real(by),
            ..p
        }),
    };
    let mut c = atom.c;
    if let Some(e) = f.exp {
        c = c * C64::new(0.0, -e.omega * by).exp();
    }
    let mut out = Vec::with_capacity(usize::from(atom.poly) + 1);
    let mut binomial = 1.0f64;
    for k in (0..=atom.poly).rev() {
        let power = f64::from(atom.poly - k);
        out.push(SpectralAtom::new(
            c.scale(binomial * (-by).powf(power)),
            Factors { poly: k, ..moved },
            Singular::Regular,
            atom.origin,
        ));
        binomial = binomial * f64::from(k) / (power + 1.0);
    }
    out
}

pub fn derive(n: SpectralSum) -> Result<SpectralSum, Left> {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in n.lanes {
        let lane = lane.expanded();
        if !lane.is_finite_sum() {
            return Err(left(
                Origin::UNKNOWN,
                Factor::Value,
                LeftReason::SeriesNonUniform,
            ));
        }
        let mut out = Lane::of(
            lane.atoms
                .iter()
                .flat_map(SpectralAtom::derivative)
                .collect(),
        );
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

/// An indicator is a pointwise factor, so it distributes over a sum however long: a cropped
/// series is the series of cropped terms, and the window is the same in every term.
pub fn crop(n: SpectralSum, window: Indicator) -> Result<SpectralSum, Left> {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in n.lanes {
        let lane = lane.expanded();
        let series = lane.series.iter().map(|s| windowed(s, window)).collect();
        let mut atoms = Vec::new();
        for atom in &lane.atoms {
            let met = atom.ind.map_or(window, |i| i.meet(window));
            if met.is_empty() {
                continue;
            }
            if let Singular::Delta { at, .. } = atom.sing {
                if met.contains(at) {
                    atoms.push(*atom);
                }
                continue;
            }
            atoms.push(atom.with(
                atom.c,
                Factors {
                    ind: Some(met),
                    ..atom.factors()
                },
            ));
        }
        let mut out = Lane {
            series,
            ..Lane::of(atoms)
        };
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

/// A hard-cropped term with its window lifted off: an indicator is a pointwise factor, so
/// what is under it reads as the closed form it was before the crop. A shoulder is not one.
pub fn crop_peeled(f: &Body) -> (Body, Option<Indicator>) {
    let Body::Crop {
        of,
        l,
        r,
        rise: 0.0,
        fall: 0.0,
    } = f
    else {
        return (f.clone(), None);
    };
    let (body, held) = crop_peeled(&of.body);
    let window = Indicator { l: *l, r: *r };
    (body, Some(held.map_or(window, |h| h.meet(window))))
}

fn windowed(s: &Series, window: Indicator) -> Series {
    Series {
        term: Part::new(
            s.term.origin,
            Body::Crop {
                of: s.term.clone(),
                l: window.l,
                r: window.r,
                rise: 0.0,
                fall: 0.0,
            },
        ),
        ..s.clone()
    }
}

/// The plateau plus two raised-cosine shoulders, seven atoms, each under its own indicator.
pub fn crop_window(
    l: Edge,
    r: Edge,
    rise: f64,
    fall: f64,
    origin: Origin,
    var: Var,
) -> SpectralSum {
    let (a, b) = (l.value(), r.value());
    let mut atoms = vec![SpectralAtom::new(
        C64::ONE,
        Factors {
            ind: Some(Indicator {
                l: Edge::at(a + rise),
                r: Edge::at(b - fall),
            }),
            ..Factors::NONE
        },
        Singular::Regular,
        origin,
    )];
    atoms.extend(shoulder(a, a + rise, rise, -0.25, origin));
    atoms.extend(shoulder(b - fall, b, fall, 0.25, origin));
    SpectralSum::of(var, vec![Lane::of(atoms)])
}

/// A shoulder of no length is a hard edge, which the plateau's own indicator already is: a
/// raised cosine over zero seconds has no half-period to turn over.
fn shoulder(from: f64, to: f64, span: f64, quarter: f64, origin: Origin) -> Vec<SpectralAtom> {
    if span <= 0.0 || !span.is_finite() {
        return Vec::new();
    }
    raised_cosine(from, to, std::f64::consts::PI / span, 0.5, quarter, origin)
}

/// `dc + 2*side*cos(w*(t - from))` over `[from, to)` and nothing outside it, as the three
/// atoms it is: the level, and one line either side of it phased to the window's own start.
pub(crate) fn raised_cosine(
    from: f64,
    to: f64,
    w: f64,
    dc: f64,
    side: f64,
    origin: Origin,
) -> Vec<SpectralAtom> {
    let window = Indicator {
        l: Edge::at(from),
        r: Edge::at(to),
    };
    let mut out = vec![SpectralAtom::new(
        C64::real(dc),
        Factors {
            ind: Some(window),
            ..Factors::NONE
        },
        Singular::Regular,
        origin,
    )];
    for sign in [1.0, -1.0] {
        out.push(SpectralAtom::new(
            C64::new(0.0, -sign * w * from).exp().scale(side),
            Factors {
                exp: Some(Exp::at(0.0, sign * w)),
                ind: Some(window),
                ..Factors::NONE
            },
            Singular::Regular,
            origin,
        ));
    }
    out
}
