// Concern: folds a written subterm, or one call of numbers, to the number it holds | Non-concern: lowering a closed form (mod.rs) | IO: (&Body, or a call's arguments) -> Option<f64>

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
