// Concern: the renderer a closed form in t becomes inside the machine that reads it, instant by instant | Non-concern: collapsing one to a buffer (sva-samples) | IO: (NodeId) -> a NodeRenderer or none

use sva_formula::{Body, Fold, NodeId, Part, Var};
use sva_samples::{Binary, NodeRenderer};

use crate::typing::Typing;

/// Numbers, `t`, arithmetic, unaries, `min`/`max`/`%`, crops, joins and components, refs
/// inlined; else `None`.
pub(crate) fn renderer(tys: &Typing, id: NodeId) -> Option<NodeRenderer> {
    let form = crate::refs::substituted_closed_form(tys, id)?;
    (form.var == Var::T).then(|| of(&form.body)).flatten()
}

fn of(f: &Body) -> Option<NodeRenderer> {
    if let Some(v) = super::constant_value(f, Var::T) {
        return Some(NodeRenderer::Const(v));
    }
    let one = |p: &Part| of(&p.body).map(Box::new);
    let each = |parts: &[Part]| {
        parts
            .iter()
            .map(|p| of(&p.body))
            .collect::<Option<Vec<_>>>()
    };
    Some(match f {
        Body::Line => NodeRenderer::Time,
        Body::Add(parts) => NodeRenderer::Add(each(parts)?),
        Body::Mul(parts) => NodeRenderer::Mul(each(parts)?),
        Body::Div(a, b) => NodeRenderer::Div(one(a)?, one(b)?),
        Body::Pow(base, n) => {
            NodeRenderer::Pow(one(base)?, Box::new(NodeRenderer::Const(f64::from(*n))))
        }
        Body::Apply(op, x) => NodeRenderer::Map((*op).into(), one(x)?),
        Body::Fold(op, parts) => {
            let [a, b] = parts.as_slice() else {
                return None;
            };
            let op = match op {
                Fold::Max => Binary::Max,
                Fold::Min => Binary::Min,
                Fold::Mod => Binary::Mod,
            };
            NodeRenderer::Zip(op, one(a)?, one(b)?)
        }
        Body::Join(parts) => NodeRenderer::Join(each(parts)?),
        Body::Channel(x, k) => NodeRenderer::Channel {
            x: one(x)?,
            k: usize::from(*k),
        },
        Body::Crop {
            of: x,
            l,
            r,
            rise,
            fall,
        } => NodeRenderer::Crop {
            x: one(x)?,
            a: l.value(),
            b: r.value(),
            rise: *rise,
            fall: *fall,
        },
        _ => return None,
    })
}
