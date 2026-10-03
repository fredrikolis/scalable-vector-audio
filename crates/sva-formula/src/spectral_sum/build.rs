// Concern: lowers a Body to the canonical atom sum | Non-concern: the atom algebra (product.rs), typing (infer.rs) | IO: (&Body, Var) -> SpectralSum or Left

use crate::affine::{Coeff, apply_scalar, exact_affine_read};
use crate::closed_form::{Body, ClosedForm, Part, Series, Var};
use crate::complex::C64;
use crate::origin::Origin;
use crate::rational::expand;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Indicator, Pole, Singular, SpectralAtom};
use crate::spectral_sum::image::{
    affine_atoms, apply, constant, crop, crop_window, delta, derive, fold, left, one,
    principal_value, shift,
};
use crate::spectral_sum::merge::simplify;
use crate::spectral_sum::product::times;
use crate::spectral_sum::{Lane, SpectralSum};
use crate::spectral_sum::{kink, spread};
use crate::through::{Opaque, Reads};

pub fn normalize_closed_form(t: &ClosedForm) -> Result<SpectralSum, Left> {
    finite(lower(&t.body, t.origin, t.var, &Opaque)?)
}

pub fn normalize(f: &Body, var: Var) -> Result<SpectralSum, Left> {
    normalize_read(f, var, &Opaque)
}

/// The same, each ref read as `reads` answers it.
pub fn normalize_read(f: &Body, var: Var, reads: &dyn Reads) -> Result<SpectralSum, Left> {
    finite(lower(f, Origin::UNKNOWN, var, reads)?)
}

/// An amplitude past the largest double has no weight to carry, so the sum refuses there.
fn finite(sum: SpectralSum) -> Result<SpectralSum, Left> {
    let unheld = sum.atoms().find(|a| !a.c.is_finite()).map(|a| a.origin);
    match unheld {
        Some(origin) => Err(Left::new(
            origin,
            AtomSketch::of(Factor::Amplitude),
            LeftReason::Overflow,
        )),
        None => Ok(sum),
    }
}

pub(crate) fn lower(
    f: &Body,
    origin: Origin,
    var: Var,
    reads: &dyn Reads,
) -> Result<SpectralSum, Left> {
    let lower_part = |p: &Part, var: Var| lower(&p.body, p.origin, var, reads);
    match f {
        Body::Const(c) => constant(var, *c, origin),
        Body::Keyed { seed, of } => {
            if crate::affine::key_moves_read(&of.body, reads) {
                return Err(left(origin, Factor::Value, LeftReason::KeyedOnASignal));
            }
            let key = sole_constant(&lower_part(of, var)?);
            match key.and_then(|c| crate::hash::draw_nearest(*seed, c.re)) {
                Some(drawn) => Ok(one(var, SpectralAtom::constant(C64::real(drawn), origin))),
                None => Err(left(origin, Factor::Value, LeftReason::Unsubstituted)),
            }
        }
        Body::Line => Ok(one(
            var,
            SpectralAtom::new(C64::ONE, Factors::poly(1), Singular::Regular, origin),
        )),
        Body::Node(id) => reads.lowered(*id, origin, var),
        Body::Index(_) | Body::Param(_) => {
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
                    Some(a) => zip(a, n, |x, y| multiply_lanes_read(x, y, reads))?,
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
            scale(lower_part(num, var)?, k.inv(), reads)
        }
        Body::Pow(base, n) => power(lower_part(base, var)?, (base.origin, *n, var), reads),
        Body::Apply(op, arg) => match apply(*op, arg, origin, var, reads) {
            Err(left) => match held_constant(arg, var, reads) {
                Some(c) => constant(var, apply_scalar(*op, c), origin),
                None => kinked(f, origin, var, left, reads),
            },
            held => held,
        },
        Body::Fold(op, args) => {
            let values = args.iter().map(|p| held_constant(p, var, reads)).collect();
            fold(*op, values, origin, var).or_else(|left| kinked(f, origin, var, left, reads))
        }
        Body::Delta { at, order } => delta(at, *order, var, reads),
        Body::Pv(at) => principal_value(at, var, reads),
        Body::Shift { by, of } => shift(lower_part(of, var)?, *by),
        Body::Warp { at, of } => match crate::affine::slide_read(&at.body, reads) {
            Some(Coeff::Exact(by)) if by.is_real() => shift(lower_part(of, var)?, -by.re),
            _ => Err(left(
                at.origin,
                Factor::Value,
                LeftReason::NonAffineArgument,
            )),
        },
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
            zip(lower_part(of, var)?, window, |x, y| {
                multiply_lanes_read(x, y, reads)
            })
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
        Body::Banded(_) => Err(left(origin, Factor::Value, LeftReason::Nonlinearity)),
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
        Body::Run(run) => {
            let atoms = run.lines().into_iter().map(|l| {
                let exp = Some(Exp::at(0.0, std::f64::consts::TAU * l.hz));
                let factors = Factors {
                    exp,
                    ..Factors::NONE
                };
                SpectralAtom::new(l.amp, factors, Singular::Regular, origin)
            });
            Ok(SpectralSum::mono(var, atoms.collect()))
        }
    }
}

/// The one number a subterm holds however deep its constants nest, so `log(max(v, e))` folds
/// where `log` and `max` each fold alone. A spelled affine constant keeps its own bits.
fn held_constant(p: &Part, var: Var, reads: &dyn Reads) -> Option<C64> {
    if let Some((a, b)) = exact_affine_read(&p.body, reads) {
        return a.is_zero().then_some(b);
    }
    if !timeless(&p.body, reads) {
        return None;
    }
    sole_constant(&lower(&p.body, p.origin, var, reads).ok()?)
}

pub(crate) fn timeless(f: &Body, reads: &dyn Reads) -> bool {
    match f {
        Body::Node(id) => reads.timeless(*id),
        Body::Line
        | Body::Index(_)
        | Body::Param(_)
        | Body::Rational(_)
        | Body::Series(_)
        | Body::Modal(_)
        | Body::Run(_)
        | Body::Delta { .. }
        | Body::Pv(_) => false,
        other => crate::closed_form::children(other)
            .iter()
            .all(|p| timeless(&p.body, reads)),
    }
}

/// A `min` or `max` of two lines is each line under its own window; a form refused whole
/// may lower half by half. Either half refused keeps the first refusal.
fn kinked(
    f: &Body,
    origin: Origin,
    var: Var,
    refused: Left,
    reads: &dyn Reads,
) -> Result<SpectralSum, Left> {
    match kink::split(f, reads) {
        Some(halves) => lower(&halves, origin, var, reads).map_err(|_| refused),
        None => Err(refused),
    }
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
    multiply_lanes_read(x, y, &Opaque)
}

/// The same, each ref a series term holds read as `reads` reads its form.
pub fn multiply_lanes_read(x: Lane, y: Lane, reads: &dyn Reads) -> Result<Lane, Left> {
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
        (true, false) => scale_lane(y, &x, reads),
        (false, true) => scale_lane(x, &y, reads),
        (false, false) => Err(left(
            Origin::UNKNOWN,
            Factor::Value,
            LeftReason::SeriesTimesSeries,
        )),
    }
}

/// A series holds its own term, so a bare gain multiplies that term here and a factor with
/// a value at each line folds into the line's own weight.
fn scale_lane(mut infinite: Lane, by: &Lane, reads: &dyn Reads) -> Result<Lane, Left> {
    let origin = by.atoms.first().map_or(Origin::UNKNOWN, |a| a.origin);
    let [gain] = by.atoms.as_slice() else {
        return per_line(infinite, by, reads);
    };
    if !gain.is_bare() || gain.is_delta() {
        return per_line(infinite, by, reads);
    }
    for s in &mut infinite.series {
        *s = scaled(s, (gain, origin), reads);
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
fn scaled(s: &Series, (gain, origin): (&SpectralAtom, Origin), reads: &dyn Reads) -> Series {
    if let Ok(mut made) = spread::times_atoms(s, std::slice::from_ref(gain), reads)
        && made.len() == 1
    {
        return made.remove(0);
    }
    let (body, window) =
        crate::spectral_sum::image::crop_peeled(&crate::through::looked(&s.term.body, reads));
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

fn per_line(mut infinite: Lane, by: &Lane, reads: &dyn Reads) -> Result<Lane, Left> {
    let mut folded = Vec::with_capacity(infinite.series.len());
    for s in &infinite.series {
        folded.extend(spread::times_atoms(s, &by.atoms, reads)?);
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

fn scale(n: SpectralSum, k: C64, reads: &dyn Reads) -> Result<SpectralSum, Left> {
    let gain = Lane::of(vec![SpectralAtom::constant(k, Origin::UNKNOWN)]);
    zip(n, SpectralSum::of(Var::T, vec![gain]), |x, y| {
        multiply_lanes_read(x, y, reads)
    })
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

fn power(
    inner: SpectralSum,
    (origin, n, var): (Origin, i32, Var),
    reads: &dyn Reads,
) -> Result<SpectralSum, Left> {
    let order = u16::try_from(n.unsigned_abs())
        .map_err(|_| left(origin, Factor::Pole, LeftReason::PoleOrder(u16::MAX)))?;
    if n >= 0 {
        let mut acc = SpectralSum::mono(inner.var, vec![SpectralAtom::constant(C64::ONE, origin)]);
        for _ in 0..order {
            acc = zip(acc, inner.clone(), |x, y| multiply_lanes_read(x, y, reads))?;
        }
        return Ok(acc);
    }
    if let Some(k) = sole_constant(&inner) {
        return reciprocal_power(k, order, var, origin);
    }
    let Some((a, b)) = affine_atoms(&inner) else {
        return Err(left(origin, Factor::Pole, LeftReason::Reciprocal));
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
            origin,
        ),
    ))
}
