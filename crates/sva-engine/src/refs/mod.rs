// Concern: what reading another node yields, per the representation it holds | Non-concern: ordering the reads (schedule.rs), collapsing a closed form (sva-samples) | IO: (NodeId, Var) -> SpectralSum

use sva_formula::closed_form::children;
use sva_formula::spectral_sum::atom::Indicator;
use sva_formula::spectral_sum::build::{multiply_lanes, sole_constant};
use sva_formula::spectral_sum::image;
use sva_formula::spectral_sum::merge::simplify;
use sva_formula::{
    Body, C64, ClosedForm, Held, Lane, Left, NodeId, Part, SpectralSum, Var, dual, inverse,
    normalize_closed_form,
};

use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::typing::{Typing, Value};

mod identity;

pub use identity::{closed_form_identity, identity, symbolic_hash};

/// The spectral sum of one node read on `want`'s axis, with every ref it holds already
/// composed in. A pair answers on either axis; anything else answers on its own.
pub fn spectral_sum_of(
    typing: &Typing,
    node: NodeId,
    want: Var,
) -> Result<SpectralSum, EngineError> {
    composed(typing, node, want, &mut Vec::new())
}

/// One node's own body, rewritten, every ref composed in; its refs may reach back to it.
pub(crate) fn spectral_sum_of_body(
    typing: &Typing,
    node: NodeId,
    body: &Body,
    var: Var,
) -> Result<SpectralSum, EngineError> {
    let folded = fold_constants(typing, body);
    compose(typing, &folded, var, &mut vec![node])
}

/// `open` is the chain of refs still being composed: a form reaching itself through
/// another is a loop no substitution closes.
fn composed(
    typing: &Typing,
    node: NodeId,
    want: Var,
    open: &mut Vec<NodeId>,
) -> Result<SpectralSum, EngineError> {
    if open.contains(&node) {
        return Err(cyclic(typing, node));
    }
    open.push(node);
    let held = match typing.value(node) {
        Value::ClosedForm(form) => {
            let body = fold_constants(typing, &form.body);
            compose(typing, &body, form.var, open)?
        }
        Value::Cast(Cast::Fourier, source) => {
            let inner = composed(typing, *source, Var::T, open)?;
            turn(typing, node, dual(&inner))?
        }
        Value::Cast(Cast::IFourier, source) => {
            let inner = composed(typing, *source, Var::F, open)?;
            turn(typing, node, inverse(&inner))?
        }
        Value::Op { name, .. } if typing.ty(node).is_closed_form() => {
            return Err(across(typing, node, name));
        }
        _ => return Err(no_closed_form(typing, node)),
    };
    open.pop();
    on_axis(typing, node, held, typing.var(node), want)
}

fn cyclic(typing: &Typing, node: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.cyclic_substitution".to_string(),
        message: format!(
            "`{}` reads itself around a loop of refs.",
            typing.name(node)
        ),
        location: Located::at(typing.name(node), None),
        help: "write the loop with self(...), which the engine classifies".to_string(),
    })
}

fn on_axis(
    typing: &Typing,
    node: NodeId,
    held: SpectralSum,
    axis: Var,
    want: Var,
) -> Result<SpectralSum, EngineError> {
    if axis == want {
        return Ok(held);
    }
    let turned = match want {
        Var::F => dual(&held),
        Var::T => inverse(&held),
    };
    turn(typing, node, turned)
}

fn turn(
    typing: &Typing,
    node: NodeId,
    turned: Result<SpectralSum, Left>,
) -> Result<SpectralSum, EngineError> {
    turned.map_err(|left| {
        EngineError::of_closed_form(
            &left.refusal(),
            typing.locate(left.origin),
            format!(
                "write `{}` inside sample(...) to leave A deliberately",
                typing.name(node)
            ),
        )
    })
}

/// A form whose operands crossed a cast is held as an operation over values, and only
/// the written form itself has atoms to compose.
fn across(typing: &Typing, node: NodeId, call: &str) -> EngineError {
    no_spectral_sum(typing.name(node), call)
}

/// An exact reading answers off a spectral sum, so a term that reaches none says which
/// subterm blocked it rather than which reading asked.
fn no_spectral_sum(node: &str, blocking: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "read.no_spectral_sum".to_string(),
        message: format!(
            "`{node}` has no spectral sum to read: `{blocking}` composes no value across a ref."
        ),
        location: Located::at(node, None),
        help: "write the construct inside the node it reads, or read it off sample(...)"
            .to_string(),
    })
}

fn no_closed_form(typing: &Typing, node: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "type.samples_in_closed_form".to_string(),
        message: format!(
            "`{}` is samples; nothing returns from samples to a closed form",
            typing.name(node)
        ),
        location: Located::at(typing.name(node), None),
        help: "read it as samples, or build the closed form without it".to_string(),
    })
}

/// Every ref naming one number, replaced by that number. A constant is the same value on
/// either axis and under every construct, so it folds where no form would substitute.
pub(crate) fn fold_constants(typing: &Typing, f: &Body) -> Body {
    folded(typing, f, &mut Vec::new())
}

fn folded(typing: &Typing, f: &Body, open: &mut Vec<NodeId>) -> Body {
    let Body::Node(id) = f else {
        return sva_formula::closed_form::map_children(f, |p| {
            Part::new(p.origin, folded(typing, &p.body, open))
        });
    };
    match number(typing, *id, open) {
        Some(c) => Body::Const(c),
        None => f.clone(),
    }
}

/// The one number a node holds, or `None` where it holds a form, samples or a ref loop.
fn number(typing: &Typing, node: NodeId, open: &mut Vec<NodeId>) -> Option<C64> {
    if open.contains(&node) {
        return None;
    }
    let Value::ClosedForm(form) = typing.value(node) else {
        return None;
    };
    open.push(node);
    let body = folded(typing, &form.body, open);
    open.pop();
    sole_constant(
        &normalize_closed_form(&ClosedForm {
            var: form.var,
            body,
            origin: form.origin,
        })
        .ok()?,
    )
}

/// A node reference is not a `Body`, so a term holding one is normalized by composing
/// the pieces around it rather than by handing the whole tree to `normalize`.
fn compose(
    typing: &Typing,
    body: &Body,
    var: Var,
    open: &mut Vec<NodeId>,
) -> Result<SpectralSum, EngineError> {
    let here = *open
        .last()
        .expect("compose runs inside the node it composes");
    if !holds_node(body) {
        return normalize_here(typing, body, var);
    }
    match body {
        Body::Node(id) => composed(typing, *id, var, open),
        Body::Add(parts) => {
            let mut lanes: Vec<Lane> = Vec::new();
            for part in parts {
                add_into(&mut lanes, compose(typing, &part.body, var, open)?);
            }
            Ok(sum(var, lanes))
        }
        Body::Mul(parts) => {
            let mut acc: Option<SpectralSum> = None;
            for part in parts {
                let next = compose(typing, &part.body, var, open)?;
                acc = Some(match acc {
                    None => next,
                    Some(held) => multiply(typing, &held, &next, var)?,
                });
            }
            Ok(acc.unwrap_or_else(|| sum(var, Vec::new())))
        }
        Body::Shift { by, of } => {
            let held = compose(typing, &of.body, var, open)?;
            image::shift(held, *by).map_err(|left| left_of(typing, left))
        }
        Body::Crop {
            of,
            l,
            r,
            rise,
            fall,
        } if *rise > 0.0 || *fall > 0.0 => {
            let held = compose(typing, &of.body, var, open)?;
            let window = image::crop_window(*l, *r, *rise, *fall, of.origin, var);
            multiply(typing, &held, &window, var)
        }
        Body::Crop { of, l, r, .. } => {
            let held = compose(typing, &of.body, var, open)?;
            image::crop(held, Indicator { l: *l, r: *r }).map_err(|left| left_of(typing, left))
        }
        Body::Div(num, den) => {
            let over = compose(typing, &den.body, var, open)?;
            let numerator = compose(typing, &num.body, var, open)?;
            multiply(typing, &numerator, &reciprocal(typing, here, &over)?, var)
        }
        Body::Join(parts) => {
            let mut lanes = Vec::new();
            for part in parts {
                lanes.extend(compose(typing, &part.body, var, open)?.lanes);
            }
            Ok(sum(var, lanes))
        }
        Body::Channel(of, k) => {
            let held = compose(typing, &of.body, var, open)?;
            match held.lanes.into_iter().nth(usize::from(*k)) {
                Some(lane) => Ok(sum(var, vec![lane])),
                None => Err(unsubstituted(typing, here, body)),
            }
        }
        // Inlined, every other construct is what it was written as, and normalizes.
        other => match inlined(typing, other, var, &mut open.clone()) {
            Some(written) => normalize_here(typing, &written, var),
            None => Err(unsubstituted(typing, here, other)),
        },
    }
}

/// A divisor a ref reaches has to be one number: a reciprocal is not an atom sum.
fn reciprocal(
    typing: &Typing,
    node: NodeId,
    over: &SpectralSum,
) -> Result<SpectralSum, EngineError> {
    let divided = || no_spectral_sum(typing.name(node), "a division by a closed form");
    let [lane] = over.lanes.as_slice() else {
        return Err(divided());
    };
    match lane.atoms.as_slice() {
        [atom] if atom.is_bare() => Ok(SpectralSum::mono(
            over.var,
            vec![sva_formula::spectral_sum::atom::SpectralAtom::constant(
                atom.c.inv(),
                atom.origin,
            )],
        )),
        _ => Err(divided()),
    }
}

fn left_of(typing: &Typing, left: Left) -> EngineError {
    EngineError::of_closed_form(
        &left.refusal(),
        typing.locate(left.origin),
        "write the subterm inside sample(...) to leave A deliberately",
    )
}

fn normalize_here(typing: &Typing, body: &Body, var: Var) -> Result<SpectralSum, EngineError> {
    normalize_closed_form(&ClosedForm {
        var,
        body: body.clone(),
        origin: sva_formula::Origin::UNKNOWN,
    })
    .map_err(|left| {
        EngineError::of_closed_form(
            &left.refusal(),
            typing.locate(left.origin),
            "write the subterm inside sample(...) to leave A deliberately",
        )
    })
}

/// A ref reaching a value no substitution inlines, under a construct with no lane rule of
/// its own: the reading has a name and nothing to read it off.
fn unsubstituted(typing: &Typing, node: NodeId, body: &Body) -> EngineError {
    no_spectral_sum(typing.name(node), named(body))
}

fn named(body: &Body) -> &'static str {
    match body {
        Body::Apply(op, _) => op.name(),
        Body::Pow(..) => "pow",
        Body::Fold(..) => "max, min or mod",
        Body::Join(_) => "join",
        Body::Channel(..) => "ch",
        Body::Series(_) => "sum",
        Body::Delta { .. } => "delta",
        Body::Pv(_) => "pv",
        Body::Deriv { .. } => "a derivative",
        Body::Warp { .. } => "a warped time",
        _ => "a construct",
    }
}

/// Every node a written form names, in written order, so a caller answers each one.
pub fn nodes_in(f: &Body) -> Vec<NodeId> {
    let mut out = Vec::new();
    collect_nodes(f, &mut out);
    out
}

fn collect_nodes(f: &Body, out: &mut Vec<NodeId>) {
    if let Body::Node(id) = f {
        if !out.contains(id) {
            out.push(*id);
        }
        return;
    }
    for part in children(f) {
        collect_nodes(&part.body, out);
    }
}

fn holds_node(f: &Body) -> bool {
    matches!(f, Body::Node(_)) || children(f).iter().any(|p| holds_node(&p.body))
}

fn sum(var: Var, mut lanes: Vec<Lane>) -> SpectralSum {
    for lane in &mut lanes {
        simplify(lane);
    }
    SpectralSum::of(var, lanes)
}

/// A width-1 operand broadcasts into every lane of the wider one, at the operator.
fn add_into(lanes: &mut Vec<Lane>, other: SpectralSum) {
    if other.lanes.is_empty() {
        return;
    }
    let width = lanes.len().max(other.lanes.len());
    if lanes.len() == 1 {
        let held = lanes[0].clone();
        lanes.resize(width, held);
    }
    for at in 0..width {
        let lane = lane_at(&other, at).clone();
        match lanes.get_mut(at) {
            Some(held) => {
                held.atoms.extend(lane.atoms);
                held.series.extend(lane.series);
                held.modal.extend(lane.modal);
            }
            None => lanes.push(lane),
        }
    }
}

fn multiply(
    typing: &Typing,
    a: &SpectralSum,
    b: &SpectralSum,
    var: Var,
) -> Result<SpectralSum, EngineError> {
    let width = a.lanes.len().max(b.lanes.len());
    let mut lanes = Vec::with_capacity(width);
    for at in 0..width {
        let held = multiply_lanes(lane_at(a, at).clone(), lane_at(b, at).clone());
        lanes.push(held.map_err(|left| {
            EngineError::of_closed_form(
                &left.refusal(),
                typing.locate(left.origin),
                "write one of the factors inside sample(...)",
            )
        })?);
    }
    Ok(sum(var, lanes))
}

fn lane_at(n: &SpectralSum, at: usize) -> &Lane {
    n.lanes.get(at).unwrap_or(&n.lanes[0])
}

/// The four ways a node reads another, one per representation the reading node holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Read {
    Substitute(Box<ClosedForm>),
    BufferHit { source: NodeId, shift: i64 },
    IndexOffset { source: NodeId, steps: i64 },
}

/// A form reading a form substitutes and allocates nothing; a sampled node reading
/// anything reads a buffer, at the index offset the call site wrote.
pub fn resolve(
    typing: &Typing,
    source: NodeId,
    steps: i64,
    reader: Held,
) -> Result<Read, EngineError> {
    if !reader.is_closed_form() {
        return Ok(match steps {
            0 => Read::BufferHit { source, shift: 0 },
            steps => Read::IndexOffset { source, steps },
        });
    }
    let form =
        substituted_closed_form(typing, source).ok_or_else(|| no_closed_form(typing, source))?;
    Ok(Read::Substitute(Box::new(form)))
}

/// Every ref inlined, where each is a form on the same axis; a crossing answers `None`.
pub fn substituted_closed_form(typing: &Typing, node: NodeId) -> Option<ClosedForm> {
    let Value::ClosedForm(form) = typing.value(node) else {
        return None;
    };
    Some(ClosedForm {
        var: form.var,
        body: substituted_body(typing, node, &form.body)?,
        origin: form.origin,
    })
}

/// The same substitution over one body of `node`'s own form.
pub(crate) fn substituted_body(typing: &Typing, node: NodeId, body: &Body) -> Option<Body> {
    let Value::ClosedForm(form) = typing.value(node) else {
        return None;
    };
    inlined(typing, body, form.var, &mut vec![node])
}

fn inlined(typing: &Typing, f: &Body, var: Var, open: &mut Vec<NodeId>) -> Option<Body> {
    match f {
        Body::Node(id) if open.contains(id) => None,
        Body::Node(id) => match typing.value(*id) {
            Value::ClosedForm(form) if form.var == var => {
                open.push(*id);
                let out = inlined(typing, &form.body, var, open);
                open.pop();
                out
            }
            _ => None,
        },
        other => {
            let mut ok = true;
            let out = sva_formula::closed_form::map_children(other, |p| {
                match inlined(typing, &p.body, var, open) {
                    Some(body) => sva_formula::Part::new(p.origin, body),
                    None => {
                        ok = false;
                        p.clone()
                    }
                }
            });
            ok.then_some(out)
        }
    }
}
