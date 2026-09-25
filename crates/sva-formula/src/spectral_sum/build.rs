// Concern: lowers a Body to the canonical atom sum | Non-concern: the atom algebra (product.rs), typing (infer.rs) | IO: (&Body, Var) -> SpectralSum or Left

use crate::closed_form::{Body, ClosedForm, Part, Series, Var};
use crate::complex::C64;
use crate::origin::Origin;
use crate::rational::expand;
use crate::refusal::{Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Factors, Indicator, Pole, Singular, SpectralAtom};
use crate::spectral_sum::image::{
    affine_atoms, apply, constant, crop, crop_window, delta, derive, fold, left, one,
    principal_value, shift,
};
use crate::spectral_sum::merge::simplify;
use crate::spectral_sum::product::times;
use crate::spectral_sum::spread;
use crate::spectral_sum::{Lane, SpectralSum};

pub fn normalize_closed_form(t: &ClosedForm) -> Result<SpectralSum, Left> {
    lower(&t.body, t.origin, t.var)
}

pub fn normalize(f: &Body, var: Var) -> Result<SpectralSum, Left> {
    lower(f, Origin::UNKNOWN, var)
}

fn lower(f: &Body, origin: Origin, var: Var) -> Result<SpectralSum, Left> {
    match f {
        Body::Const(c) => constant(var, *c, origin),
        Body::Keyed { seed, of } => {
            if crate::affine::key_moves(&of.body) {
                return Err(left(origin, Factor::Value, LeftReason::KeyedOnASignal));
            }
            match sole_constant(&lower_part(of, var)?) {
                Some(c) => Ok(one(
                    var,
                    SpectralAtom::constant(C64::real(crate::hash::draw(*seed, c.re)), origin),
                )),
                None => Err(left(origin, Factor::Value, LeftReason::Unsubstituted)),
            }
        }
        Body::Line => Ok(one(
            var,
            SpectralAtom::new(C64::ONE, Factors::poly(1), Singular::Regular, origin),
        )),
        Body::Index(_) | Body::Param(_) | Body::Node(_) => {
            Err(left(origin, Factor::Value, LeftReason::Unsubstituted))
        }
        Body::Add(parts) => {
            let mut acc: Option<SpectralSum> = None;
            for p in parts {
                let n = lower_part(p, var)?;
                acc = Some(match acc {
                    None => n,
                    Some(a) => zip(a, n, |x, y| {
                        let mut lane = Lane {
                            atoms: [x.atoms, y.atoms].concat(),
                            series: [x.series, y.series].concat(),
                            modal: [x.modal, y.modal].concat(),
                        };
                        simplify(&mut lane);
                        Ok(lane)
                    })?,
                });
            }
            Ok(acc.unwrap_or_else(|| one(var, SpectralAtom::constant(C64::ZERO, origin))))
        }
        Body::Mul(parts) => {
            let mut acc: Option<SpectralSum> = None;
            for p in parts {
                let n = lower_part(p, var)?;
                acc = Some(match acc {
                    None => n,
                    Some(a) => zip(a, n, multiply_lanes)?,
                });
            }
            Ok(acc.unwrap_or_else(|| one(var, SpectralAtom::constant(C64::ONE, origin))))
        }
        Body::Div(num, den) => {
            let d = lower_part(den, var)?;
            let Some(k) = sole_constant(&d) else {
                return Err(left(den.origin, Factor::Value, LeftReason::Reciprocal));
            };
            if k.is_zero() {
                return Err(left(den.origin, Factor::Value, LeftReason::NoValue));
            }
            scale(lower_part(num, var)?, k.inv())
        }
        Body::Pow(base, n) => power(base, *n, var),
        Body::Apply(op, arg) => apply(*op, arg, origin, var),
        Body::Fold(op, args) => fold(*op, args, origin, var),
        Body::Delta { at, order } => delta(at, *order, var),
        Body::Pv(at) => principal_value(at, var),
        Body::Shift { by, of } => shift(lower_part(of, var)?, *by),
        Body::Warp { at, .. } => Err(left(
            at.origin,
            Factor::Value,
            LeftReason::NonAffineArgument,
        )),
        Body::Deriv { order, of } => {
            let mut n = lower_part(of, var)?;
            for _ in 0..*order {
                n = derive(n)?;
            }
            Ok(n)
        }
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } if *rise > 0.0 || *fall > 0.0 => {
            let window = crop_window(*l, *r, *rise, *fall, of.origin, var);
            zip(lower_part(of, var)?, window, multiply_lanes)
        }
        Body::Crop { of, l, r, .. } => crop(lower_part(of, var)?, Indicator { l: *l, r: *r }),
        Body::Join(parts) => {
            let mut lanes = Vec::new();
            for p in parts {
                lanes.extend(lower_part(p, var)?.lanes);
            }
            Ok(SpectralSum::of(var, lanes))
        }
        Body::Channel(of, k) => {
            let n = lower_part(of, var)?;
            match n.lanes.into_iter().nth(usize::from(*k)) {
                Some(lane) => Ok(SpectralSum::of(var, vec![lane])),
                None => Err(left(of.origin, Factor::Value, LeftReason::Unsubstituted)),
            }
        }
        Body::Rational(r) => Ok(SpectralSum::mono(var, expand(r, origin)?)),
        Body::Series(s) => Ok(SpectralSum::of(
            var,
            vec![Lane {
                series: vec![(**s).clone()],
                ..Lane::default()
            }],
        )),
        Body::Modal(m) => Ok(SpectralSum::of(
            var,
            vec![Lane {
                modal: vec![m.clone()],
                ..Lane::default()
            }],
        )),
    }
}

fn lower_part(p: &Part, var: Var) -> Result<SpectralSum, Left> {
    lower(&p.body, p.origin, var)
}

/// One operand of width 1 broadcasts against a wide one; equal widths pair off.
fn zip(
    a: SpectralSum,
    b: SpectralSum,
    mut op: impl FnMut(Lane, Lane) -> Result<Lane, Left>,
) -> Result<SpectralSum, Left> {
    let var = a.var;
    let width = a.width().max(b.width());
    let mut lanes = Vec::with_capacity(width);
    for k in 0..width {
        let x = a.lanes[k % a.width().max(1)].clone();
        let y = b.lanes[k % b.width().max(1)].clone();
        lanes.push(op(x, y)?);
    }
    Ok(SpectralSum::of(var, lanes))
}

pub fn multiply_lanes(x: Lane, y: Lane) -> Result<Lane, Left> {
    let (x, y) = (x.expanded(), y.expanded());
    match (x.is_finite_sum(), y.is_finite_sum()) {
        (true, true) => {
            let mut atoms = Vec::new();
            for a in &x.atoms {
                for b in &y.atoms {
                    atoms.extend(times(a, b)?);
                }
            }
            let mut lane = Lane::of(atoms);
            simplify(&mut lane);
            Ok(lane)
        }
        (true, false) => scale_lane(y, &x),
        (false, true) => scale_lane(x, &y),
        (false, false) => Err(left(
            Origin::UNKNOWN,
            Factor::Value,
            LeftReason::SeriesTimesSeries,
        )),
    }
}

/// A series holds its own term, so a bare gain multiplies that term here and a factor with
/// a value at each line folds into the line's own weight.
fn scale_lane(mut infinite: Lane, by: &Lane) -> Result<Lane, Left> {
    let origin = by.atoms.first().map_or(Origin::UNKNOWN, |a| a.origin);
    let [gain] = by.atoms.as_slice() else {
        return per_line(infinite, by);
    };
    if !gain.is_bare() || gain.is_delta() {
        return per_line(infinite, by);
    }
    for s in &mut infinite.series {
        *s = scaled(s, gain, origin);
    }
    infinite.atoms = infinite
        .atoms
        .iter()
        .map(|a| a.with(a.c * gain.c, a.factors()))
        .collect();
    Ok(infinite)
}

/// A gain belongs in each line's own weight, where the term still reads as a line closed form. A
/// term no line closed form reads takes the gain as a written factor instead.
fn scaled(s: &Series, gain: &SpectralAtom, origin: Origin) -> Series {
    if let Ok(mut made) = spread::times_atoms(s, std::slice::from_ref(gain))
        && made.len() == 1
    {
        return made.remove(0);
    }
    let (body, window) = crate::spectral_sum::image::crop_peeled(&s.term.body);
    let held = Body::Mul(vec![
        Part::new(origin, Body::Const(gain.c)),
        Part::new(s.term.origin, body),
    ]);
    Series {
        term: Part::new(
            s.term.origin,
            match window {
                Some(window) => Body::Crop {
                    of: Part::new(s.term.origin, held),
                    l: window.l,
                    r: window.r,
                    rise: 0.0,
                    fall: 0.0,
                },
                None => held,
            },
        ),
        ..s.clone()
    }
}

fn per_line(mut infinite: Lane, by: &Lane) -> Result<Lane, Left> {
    let mut folded = Vec::with_capacity(infinite.series.len());
    for s in &infinite.series {
        folded.extend(spread::times_atoms(s, &by.atoms)?);
    }
    infinite.series = folded;
    let mut atoms = Vec::new();
    for a in &infinite.atoms {
        for b in &by.atoms {
            atoms.extend(times(a, b)?);
        }
    }
    infinite.atoms = atoms;
    Ok(infinite)
}

/// A constant base has no pole to place: `k^-n` is the number `1/k^n`, the reciprocal a pole's
/// own weight takes. A zero base names no number, exactly as `1/0` does.
fn reciprocal_power(k: C64, order: u16, var: Var, origin: Origin) -> Result<SpectralSum, Left> {
    let magnitude = k.powi(u32::from(order));
    if magnitude.is_zero() {
        return Err(left(origin, Factor::Value, LeftReason::NoValue));
    }
    Ok(one(var, SpectralAtom::constant(magnitude.inv(), origin)))
}

fn scale(n: SpectralSum, k: C64) -> Result<SpectralSum, Left> {
    let gain = Lane::of(vec![SpectralAtom::constant(k, Origin::UNKNOWN)]);
    zip(n, SpectralSum::of(Var::T, vec![gain]), multiply_lanes)
}

/// The one number a spectral sum holds, where it holds one and no atom of it is singular.
pub fn sole_constant(n: &SpectralSum) -> Option<C64> {
    match n.lanes.as_slice() {
        [lane] if lane.is_finite_sum() => match lane.atoms.as_slice() {
            [a] if a.is_bare() && !a.is_delta() => Some(a.c),
            [] => Some(C64::ZERO),
            _ => None,
        },
        _ => None,
    }
}

fn power(base: &Part, n: i32, var: Var) -> Result<SpectralSum, Left> {
    let inner = lower_part(base, var)?;
    let order = u16::try_from(n.unsigned_abs())
        .map_err(|_| left(base.origin, Factor::Pole, LeftReason::PoleOrder(u16::MAX)))?;
    if n >= 0 {
        let mut acc = SpectralSum::mono(
            inner.var,
            vec![SpectralAtom::constant(C64::ONE, base.origin)],
        );
        for _ in 0..order {
            acc = zip(acc, inner.clone(), multiply_lanes)?;
        }
        return Ok(acc);
    }
    if let Some(k) = sole_constant(&inner) {
        return reciprocal_power(k, order, var, base.origin);
    }
    let Some((a, b)) = affine_atoms(&inner) else {
        return Err(left(base.origin, Factor::Pole, LeftReason::Reciprocal));
    };
    Ok(one(
        var,
        SpectralAtom::new(
            a.powi(u32::from(order)).inv(),
            Factors {
                pole: Some(Pole {
                    at: -b / a,
                    order,
                    pv: false,
                }),
                ..Factors::NONE
            },
            Singular::Regular,
            base.origin,
        ),
    ))
}
