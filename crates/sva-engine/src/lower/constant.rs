// Concern: folds a written subterm, or one call of numbers, to the number it holds | Non-concern: lowering a closed form (mod.rs) | IO: (&Body, or a call's arguments) -> Option<f64>

use sva_formula::affine::apply_scalar;
use sva_formula::closed_form::children;
use sva_formula::{Body, C64, ClosedForm, Fold, Origin, Part, Var, normalize_closed_form};

pub fn is_constant(f: &Body) -> bool {
    match f {
        Body::Line | Body::Node(_) | Body::Param(_) | Body::Index(_) => false,
        other => children(other).iter().all(|p| is_constant(&p.body)),
    }
}

/// One bare atom is a number, an empty lane the zero it folded to; else it is not.
pub fn constant_value(body: &Body, var: Var) -> Option<f64> {
    if !is_constant(body) {
        return None;
    }
    let form = ClosedForm {
        var,
        body: body.clone(),
        origin: Origin::UNKNOWN,
    };
    let sum = normalize_closed_form(&form).ok()?;
    let [lane] = sum.lanes.as_slice() else {
        return None;
    };
    match lane.atoms.as_slice() {
        [] => Some(0.0),
        [atom] => atom.is_bare().then_some(atom.c.re),
        _ => None,
    }
}

pub fn constant_modulo(a: f64, b: f64) -> Option<f64> {
    constant_value(&Body::Fold(Fold::Mod, vec![number(a), number(b)]), Var::T)
}

/// Through what `arithmetic` builds; a name it omits names no number.
pub fn constant_call(name: &str, positional: &[f64], named: &[(&str, f64)]) -> Option<f64> {
    let bodies: Vec<Body> = positional
        .iter()
        .map(|x| Body::Const(C64::real(*x)))
        .collect();
    let folded = super::calls::arithmetic(name, &bodies, named, Var::T, Origin::UNKNOWN)?;
    constant_value(&folded, Var::T)
}

fn number(x: f64) -> Part {
    Part::new(Origin::UNKNOWN, Body::Const(C64::real(x)))
}

/// A built body of numbers, one infinite, as IEEE arithmetic: no atom holds `inf`.
pub fn unbounded(body: &Body) -> Option<f64> {
    (is_constant(body) && holds_infinite(body))
        .then(|| scalar(body))
        .flatten()
}

pub fn holds_infinite(f: &Body) -> bool {
    matches!(f, Body::Const(c) if !c.is_finite())
        || children(f).iter().any(|p| holds_infinite(&p.body))
}

fn scalar(f: &Body) -> Option<f64> {
    let of = |p: &Part| scalar(&p.body);
    Some(match f {
        Body::Const(c) if c.im == 0.0 => c.re,
        Body::Add(parts) => parts.iter().try_fold(0.0, |a, p| Some(a + of(p)?))?,
        Body::Mul(parts) => parts.iter().try_fold(1.0, |a, p| Some(a * of(p)?))?,
        Body::Div(a, b) => of(a)? / of(b)?,
        Body::Pow(base, n) => of(base)?.powi(*n),
        Body::Apply(op, x) => apply_scalar(*op, C64::real(of(x)?)).re,
        Body::Fold(op, parts) => match (op, parts.as_slice()) {
            (Fold::Max, [a, b]) => of(a)?.max(of(b)?),
            (Fold::Min, [a, b]) => of(a)?.min(of(b)?),
            (Fold::Mod, [a, b]) => of(a)?.rem_euclid(of(b)?),
            _ => return None,
        },
        _ => return None,
    })
}

/// The one number a node holds at `rate`, `inf` included: a constant closed form, or a grid
/// count, which is a number of seconds once a rate is named.
pub(crate) fn number_at(
    tys: &crate::typing::Typing,
    id: sva_formula::NodeId,
    rate: u32,
) -> Option<f64> {
    match tys.value(id) {
        crate::typing::Value::ClosedForm(form) => match form.body {
            Body::Const(c) if c.im == 0.0 => Some(c.re),
            _ => constant_value(&form.body, form.var),
        },
        crate::typing::Value::Grid(count) => Some(count / f64::from(rate)),
        _ => None,
    }
}
