// Concern: where a solver's automation switches on its clock, and its identity with each switch not yet reached undone | Non-concern: storing runs under it | IO: (NodeId) -> (sample, Hash) per switch

use std::collections::BTreeSet;

use sva_formula::closed_form::{children, map_children};
use sva_formula::{Body, C64, ClosedForm, Edge, Hash, NodeId, Part, Through, Var};
use sva_samples::Grid;

use super::identity::{identity, solver};
use super::{PerNode, read_through, reads_through};
use crate::error::EngineError;
use crate::lower::field;
use crate::render::table::support::window;
use crate::typing::{Typing, Value};

/// Each switch a solver's arguments make, and the solver's identity before it: a note released
/// at `r` is the held note up to `r`. Empty for anything but a solver.
pub(crate) fn switches(typing: &Typing, id: NodeId) -> Result<Vec<(i64, Hash)>, EngineError> {
    let Value::Solver { params, varying } = typing.value(id) else {
        return Ok(Vec::new());
    };
    let grid = typing.grid(id);
    let mut points = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for (_, arg) in varying {
        if let Value::ClosedForm(form) = typing.value(*arg)
            && reads_through(typing, &form.body, form.var)
        {
            body_points(typing, &form.body, grid, (&mut points, &mut seen));
        }
    }
    let mut out = Vec::with_capacity(points.len());
    for at in points {
        let mut held = (**params).clone();
        let mut read = Vec::new();
        for (key, arg) in varying {
            match argument_before(typing, *arg, at, grid)? {
                Argued::Number(v) => *field(&mut held, key).expect("a varying field") = v,
                Argued::Node(h) => read.push((*key, h)),
            }
        }
        out.push((at, solver(&held, &read)));
    }
    Ok(out)
}

enum Argued {
    Number(f64),
    Node(Hash),
}

fn argument_before(
    typing: &Typing,
    arg: NodeId,
    at: i64,
    grid: Grid,
) -> Result<Argued, EngineError> {
    let form = match typing.value(arg) {
        Value::ClosedForm(form) if reads_through(typing, &form.body, form.var) => form,
        _ => return identity(typing, arg).map(Argued::Node),
    };
    read_through(typing, |through| {
        let before = Before {
            through,
            at,
            grid,
            stood: PerNode::new(),
        };
        let body = before.body(&form.body);
        let written = Written {
            typing,
            through,
            numbers: PerNode::new(),
            named: PerNode::new(),
        };
        Ok(match written.constant_value(&body, form.var) {
            Some(v) => Argued::Number(v + 0.0),
            None => Argued::Node(written.written(&ClosedForm { body, ..*form })?),
        })
    })
}

fn opens(grid: Grid, l: f64) -> Option<i64> {
    l.is_finite().then(|| window(grid, l, f64::INFINITY).start)
}

/// Before the first sample at or past `r - fall` a crop's gain is the one it has with no end;
/// a fall's own rounding moves that one sample earlier.
fn closes(grid: Grid, r: f64, fall: f64) -> Option<i64> {
    let edge = r - fall;
    let first = edge
        .is_finite()
        .then(|| window(grid, edge, f64::INFINITY).start)?;
    Some(match fall > 0.0 {
        true => first - 1,
        false => first,
    })
}

fn body_points(
    typing: &Typing,
    f: &Body,
    grid: Grid,
    (out, seen): (&mut BTreeSet<i64>, &mut BTreeSet<NodeId>),
) {
    match f {
        Body::Shift { .. } | Body::Warp { .. } => {}
        Body::Node(id) => {
            if seen.insert(*id)
                && let Value::ClosedForm(form) = typing.value(*id)
            {
                body_points(typing, &form.body, grid, (out, seen));
            }
        }
        Body::Crop { of, l, r, fall, .. } => {
            out.extend(opens(grid, l.value()));
            out.extend(closes(grid, r.value(), *fall));
            body_points(typing, &of.body, grid, (out, seen));
        }
        other => {
            for p in children(other) {
                body_points(typing, &p.body, grid, (&mut *out, &mut *seen));
            }
        }
    }
}

/// A form with every crop opening at or past `at` shut and every one closing there left
/// open, each ref it rewrites standing in as a node of its own, once per node.
struct Before<'a, 'w> {
    through: &'a Through<'w>,
    at: i64,
    grid: Grid,
    stood: PerNode<Option<NodeId>>,
}

impl Before<'_, '_> {
    fn body(&self, f: &Body) -> Body {
        let (at, grid) = (self.at, self.grid);
        match f {
            Body::Shift { .. } | Body::Warp { .. } => f.clone(),
            Body::Node(id) => Body::Node(self.node(*id).unwrap_or(*id)),
            Body::Crop {
                of,
                l,
                r,
                rise,
                fall,
            } => {
                if opens(grid, l.value()).is_some_and(|c| c >= at) {
                    return Body::Const(C64::ZERO);
                }
                let inner = Part::new(of.origin, self.body(&of.body));
                match closes(grid, r.value(), *fall) {
                    Some(c) if c >= at => Body::Crop {
                        of: inner,
                        l: *l,
                        r: Edge::PosInf,
                        rise: *rise,
                        fall: 0.0,
                    },
                    _ => Body::Crop {
                        of: inner,
                        l: *l,
                        r: *r,
                        rise: *rise,
                        fall: *fall,
                    },
                }
            }
            other => map_children(other, |p| Part::new(p.origin, self.body(&p.body))),
        }
    }

    fn node(&self, id: NodeId) -> Option<NodeId> {
        self.stood.of(id, || {
            let rewritten = self
                .through
                .read(id, |form| Some(self.body(form)).filter(|body| body != form));
            rewritten.flatten().map(|body| self.through.stand(body))
        })
    }
}

/// What a rewritten form comes to, as the typing takes the same form written as a solver's
/// argument: the number it holds, refs naming numbers folded in, or what names it.
struct Written<'a, 'w> {
    typing: &'a Typing,
    through: &'a Through<'w>,
    numbers: PerNode<Option<C64>>,
    named: PerNode<Hash>,
}

impl Written<'_, '_> {
    fn constant_value(&self, body: &Body, var: Var) -> Option<f64> {
        let folded = super::folded_by(body, &mut |id| self.number(id, var));
        crate::lower::constant_value(&folded, var)
    }

    /// A stand-in holds a number as the node it stands for would.
    fn number(&self, id: NodeId, var: Var) -> Option<C64> {
        match Through::stands(id) {
            false => super::number_of(self.typing, id),
            true => self.numbers.of(id, || {
                let body = self.through.read(id, Body::clone)?;
                let folded = super::folded_by(&body, &mut |id| self.number(id, var));
                super::sole_number(&ClosedForm {
                    var,
                    body: folded.into_owned(),
                    origin: sva_formula::Origin::UNKNOWN,
                })
            }),
        }
    }

    /// Each stand-in named by the form it stands for, on the variable of the form it rewrote.
    fn written(&self, form: &ClosedForm) -> Result<Hash, EngineError> {
        let var = form.var;
        super::identity::written(form, &mut |id| match Through::stands(id) {
            false => Ok((identity(self.typing, id)?, self.typing.var(id))),
            true => {
                let held = self.named.try_of(id, || {
                    let body = self.through.read(id, Body::clone);
                    self.written(&ClosedForm {
                        var,
                        body: body.expect("a stand-in's form"),
                        origin: sva_formula::Origin::UNKNOWN,
                    })
                })?;
                Ok((held, var))
            }
        })
    }
}
