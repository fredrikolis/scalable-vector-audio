// Concern: where a solver's automation switches on its clock, and its identity with each switch not yet reached undone | Non-concern: storing runs under it | IO: (NodeId) -> (sample, Hash) per switch

use std::collections::BTreeSet;

use sva_formula::closed_form::{children, map_children};
use sva_formula::{
    Body, C64, ClosedForm, Edge, Hash, NodeId, Part, Through, Var, hash_closed_form_with,
    hash_spectral_sum, normalize_read,
};
use sva_samples::Grid;

use super::identity::{Sink, identity};
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
        for (key, _) in &read {
            *field(&mut held, key).expect("a varying field") = f64::NAN;
        }
        let mut sink = Sink::new();
        sink.text(&format!("{held:?}"));
        for (key, h) in read {
            sink.text(key);
            sink.hash(h);
        }
        out.push((at, sink.finish()));
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
            constant: PerNode::new(),
            series: PerNode::new(),
            named: PerNode::new(),
        };
        Ok(match written.constant_value(&body, form.var) {
            Some(v) => Argued::Number(v + 0.0),
            None => Argued::Node(written.identity(&ClosedForm { body, ..*form })?),
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

/// What a rewritten form comes to: the number it holds, or what names it.
struct Written<'a, 'w> {
    typing: &'a Typing,
    through: &'a Through<'w>,
    constant: PerNode<bool>,
    series: PerNode<bool>,
    named: PerNode<Hash>,
}

impl Written<'_, '_> {
    /// One bare atom is a number, an empty lane the zero it folded to; a finite series in it is
    /// summed term by term first.
    fn constant_value(&self, body: &Body, var: Var) -> Option<f64> {
        if !self.is_constant(body) {
            return None;
        }
        if self.holds_series(body) {
            let whole = sva_formula::through::written_out(body, self.through);
            return crate::lower::constant_value(&whole, var);
        }
        let sum = normalize_read(body, var, self.through).ok()?;
        crate::lower::constant_of(&sum)
    }

    fn is_constant(&self, f: &Body) -> bool {
        let node = |id: NodeId| {
            self.constant.of(id, || {
                let read = self.through.read(id, |form| self.is_constant(form));
                read.unwrap_or(false)
            })
        };
        crate::lower::constant_with(f, &node)
    }

    fn holds_series(&self, f: &Body) -> bool {
        match f {
            Body::Series(_) => true,
            Body::Node(id) => self.series.of(*id, || {
                let read = self.through.read(*id, |form| self.holds_series(form));
                read.unwrap_or(false)
            }),
            other => children(other).iter().any(|p| self.holds_series(&p.body)),
        }
    }

    /// Its spectral sum where it has one, so two spellings of one form are one value; else its
    /// written form, each ref named by what it is.
    fn identity(&self, form: &ClosedForm) -> Result<Hash, EngineError> {
        match normalize_read(&form.body, form.var, self.through) {
            Ok(sum) => Ok(hash_spectral_sum(&sum)),
            Err(_) => self.written(form),
        }
    }

    fn written(&self, form: &ClosedForm) -> Result<Hash, EngineError> {
        let mut refused = None;
        let hash = hash_closed_form_with(form, &mut |id| match self.named(id, form.var) {
            Ok(held) => held,
            Err(e) => {
                refused.get_or_insert(e);
                Hash(0, 0)
            }
        });
        refused.map_or(Ok(hash), Err)
    }

    fn named(&self, id: NodeId, var: Var) -> Result<Hash, EngineError> {
        self.named.try_of(id, || match Through::stands(id) {
            false => identity(self.typing, id),
            true => {
                let body = self.through.read(id, Body::clone);
                self.written(&ClosedForm {
                    var,
                    body: body.expect("a stand-in's form"),
                    origin: sva_formula::Origin::UNKNOWN,
                })
            }
        })
    }
}
