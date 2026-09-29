// Concern: the renderer a closed form in t becomes inside the machine that reads it, instant by instant | Non-concern: collapsing one to a buffer (sva-samples) | IO: (NodeId) -> a NodeRenderer or none

use sva_formula::{Body, Fold, NodeId, Part, Var};
use sva_samples::{Binary, NodeRenderer, Wrap};

use crate::time::{Affine, Q};
use crate::typing::Typing;

/// Numbers, `t`, arithmetic, unaries, `min`/`max`/`%`, finite sums written out, crops, joins
/// and components, refs inlined; else `None`.
pub(crate) fn renderer(tys: &Typing, id: NodeId) -> Option<NodeRenderer> {
    let form = crate::refs::substituted_closed_form(tys, id)?;
    (form.var == Var::T).then(|| of(&form.body)).flatten()
}

fn of(f: &Body) -> Option<NodeRenderer> {
    if let Some(v) = super::constant_value(f, Var::T) {
        return Some(NodeRenderer::Const(v));
    }
    if let Some(wrap) = wrapped(f) {
        return Some(NodeRenderer::Wrap(wrap));
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
        Body::Series(_) => return of(&sva_formula::series::written_out(f)?),
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

/// Whether the machine computes `f` as one exact `Wrap`, rounded once.
pub(crate) fn wraps(f: &Body) -> bool {
    wrapped(f).is_some()
}

/// A line in `t` plus a multiple of one line modulo a positive number, each exact as
/// `loops::time_of` reads a time; `None` where no modulo is in it, or it is no such sum.
fn wrapped(f: &Body) -> Option<Wrap> {
    let Exact {
        line,
        wrap: Some((gain, inner, period)),
    } = exact(f)?
    else {
        return None;
    };
    let pair = |q: Q| (q.num(), q.den());
    Some(Wrap {
        scale: pair(line.scale),
        shift: pair(line.shift),
        gain: pair(gain),
        inner: [pair(inner.scale), pair(inner.shift)],
        period: pair(period),
    })
}

/// `line + gain*(inner mod period)`.
#[derive(Clone, Copy, PartialEq)]
struct Exact {
    line: Affine,
    wrap: Option<(Q, Affine, Q)>,
}

impl Exact {
    fn number(&self) -> Option<Q> {
        (self.line.scale.is_zero() && self.wrap.is_none()).then_some(self.line.shift)
    }

    fn scaled(self, k: Q) -> Option<Exact> {
        Some(Exact {
            line: Affine {
                scale: self.line.scale.mul(k)?,
                shift: self.line.shift.mul(k)?,
            },
            wrap: match self.wrap {
                Some((gain, inner, period)) => Some((gain.mul(k)?, inner, period)),
                None => None,
            },
        })
    }

    fn plus(self, o: Exact) -> Option<Exact> {
        let wrap = match (self.wrap, o.wrap) {
            (None, w) | (w, None) => w,
            (Some((g, inner, p)), Some((h, other, q))) if inner == other && p == q => {
                Some((g.add(h)?, inner, p))
            }
            _ => return None,
        };
        Some(Exact {
            line: Affine {
                scale: self.line.scale.add(o.line.scale)?,
                shift: self.line.shift.add(o.line.shift)?,
            },
            wrap,
        })
    }
}

fn exact(f: &Body) -> Option<Exact> {
    let number = |shift| Exact {
        line: Affine {
            scale: Q::ZERO,
            shift,
        },
        wrap: None,
    };
    match f {
        Body::Line => Some(Exact {
            line: Affine::NOW,
            wrap: None,
        }),
        Body::Const(c) if c.im == 0.0 => Some(number(Q::decimal(c.re)?)),
        Body::Add(parts) => parts
            .iter()
            .try_fold(number(Q::ZERO), |sum, p| sum.plus(exact(&p.body)?)),
        Body::Mul(parts) => {
            let (mut k, mut rest) = (Q::ONE, None);
            for p in parts {
                let factor = exact(&p.body)?;
                match (factor.number(), rest) {
                    (Some(n), _) => k = k.mul(n)?,
                    (None, None) => rest = Some(factor),
                    (None, Some(_)) => return None,
                }
            }
            rest.unwrap_or(number(Q::ONE)).scaled(k)
        }
        Body::Div(a, b) => exact(&a.body)?.scaled(Q::ONE.div(exact(&b.body)?.number()?)?),
        Body::Fold(Fold::Mod, parts) => {
            let [x, p] = parts.as_slice() else {
                return None;
            };
            let (x, p) = (exact(&x.body)?, exact(&p.body)?.number()?);
            (x.wrap.is_none() && !x.line.scale.is_zero() && p > Q::ZERO).then_some(Exact {
                line: Affine {
                    scale: Q::ZERO,
                    shift: Q::ZERO,
                },
                wrap: Some((Q::ONE, x.line, p)),
            })
        }
        _ => None,
    }
}
