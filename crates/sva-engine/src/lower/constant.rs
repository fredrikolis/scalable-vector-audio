// Concern: folds a written subterm, or one call of numbers, to the number it holds | Non-concern: lowering a closed form (mod.rs) | IO: (&Body, or a call's arguments) -> Option<f64>

use sva_formula::affine::apply_scalar;
use sva_formula::closed_form::children;
use sva_formula::{Body, C64, ClosedForm, Fold, NodeId, Origin, Part, Var, normalize_closed_form};

pub fn is_constant(f: &Body) -> bool {
    constant_with(f, &|_| false)
}

/// The same, each ref answered by `node`.
pub(crate) fn constant_with(f: &Body, node: &dyn Fn(NodeId) -> bool) -> bool {
    match f {
        Body::Node(id) => node(*id),
        Body::Line | Body::Param(_) | Body::Index(_) => false,
        other => children(other).iter().all(|p| constant_with(&p.body, node)),
    }
}

/// One bare atom is a number, an empty lane the zero it folded to; else it is not.
pub fn constant_value(body: &Body, var: Var) -> Option<f64> {
    if !is_constant(body) {
        return None;
    }
    let form = ClosedForm {
        var,
        body: sva_formula::series::written_out(body)?,
        origin: Origin::UNKNOWN,
    };
    constant_of(&normalize_closed_form(&form).ok()?)
}

pub fn constant_of(sum: &sva_formula::SpectralSum) -> Option<f64> {
    let [lane] = sum.lanes.as_slice() else {
        return None;
    };
    if !lane.series.is_empty() || !lane.modal.is_empty() {
        return None;
    }
    match lane.atoms.as_slice() {
        [] => Some(0.0),
        [atom] => atom.is_bare().then_some(atom.c.re),
        _ => None,
    }
}

pub fn constant_modulo(a: f64, b: f64) -> Option<f64> {
    folded_number(&Body::Fold(Fold::Mod, vec![number(a), number(b)]))
}

/// Through what `arithmetic` builds; a name it omits names no number.
pub fn constant_call(name: &str, positional: &[f64], named: &[(&str, f64)]) -> Option<f64> {
    let bodies: Vec<Body> = positional
        .iter()
        .map(|x| Body::Const(C64::real(*x)))
        .collect();
    folded_number(&super::calls::arithmetic(
        name,
        bodies,
        named,
        Var::T,
        Origin::UNKNOWN,
    )?)
}

fn folded_number(body: &Body) -> Option<f64> {
    unbounded(body).or_else(|| constant_value(body, Var::T))
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

/// A sum whose numbers add to `inf` and whose every other term is a real polynomial in the free
/// variable, finite wherever it reaches up to `reach`: IEEE arithmetic makes it that `inf`.
pub fn swamped(body: &Body, reach: f64) -> Option<f64> {
    let Body::Add(parts) = body else {
        return None;
    };
    let (numbers, terms): (Vec<&Part>, Vec<&Part>) =
        parts.iter().partition(|p| is_constant(&p.body));
    let sum = numbers
        .iter()
        .try_fold(0.0, |held, p| Some(held + scalar(&p.body)?))?;
    let finite = |p: &&Part| {
        let coeffs = sva_formula::affine::polynomial(&p.body)?;
        coeffs.iter().enumerate().try_fold(0.0f64, |held, (k, c)| {
            let c = c.exact().filter(|c| c.im == 0.0 && c.re.is_finite())?;
            Some(held + c.re.abs() * reach.powi(k as i32))
        })
    };
    let reached = terms
        .iter()
        .try_fold(0.0, |held, p| Some(held + finite(p)?));
    (sum.is_infinite() && reached.is_some_and(|r| r < f64::MAX / 4.0)).then_some(sum)
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

/// The one number a node holds, `inf` included.
pub(crate) fn number_of(tys: &crate::typing::Typing, id: sva_formula::NodeId) -> Option<f64> {
    match tys.value(id) {
        crate::typing::Value::ClosedForm(form) => match form.body {
            Body::Const(c) if c.im == 0.0 => Some(c.re),
            _ => constant_value(&form.body, form.var),
        },
        _ => None,
    }
}
