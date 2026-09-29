// Concern: where a solver's automation switches on its clock, and its identity with each switch not yet reached undone | Non-concern: storing runs under it | IO: (NodeId) -> (sample, Hash) per switch

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::closed_form::map_children;
use sva_formula::{Body, C64, ClosedForm, Edge, Hash, NodeId, Part};
use sva_samples::Grid;

use super::identity::{Sink, formula_identity, identity_in};
use super::substituted_closed_form;
use crate::error::EngineError;
use crate::lower::field;
use crate::render::table::support::window;
use crate::typing::{Typing, Value};

/// Each switch a solver's arguments make, and the solver's identity before it: a note released
/// at `r` is the held note up to `r`. Empty for anything but a solver.
pub(crate) fn switches(
    typing: &Typing,
    id: NodeId,
    named: &mut BTreeMap<NodeId, Hash>,
) -> Result<Vec<(i64, Hash)>, EngineError> {
    let Value::Solver { params, varying } = typing.value(id) else {
        return Ok(Vec::new());
    };
    let grid = typing.grid(id);
    let mut points = BTreeSet::new();
    for (_, arg) in varying {
        if let Some(form) = substituted_closed_form(typing, *arg) {
            body_points(&form.body, grid, &mut points);
        }
    }
    let mut out = Vec::with_capacity(points.len());
    for at in points {
        let mut held = (**params).clone();
        let mut read = Vec::new();
        for (key, arg) in varying {
            match argument_before(typing, *arg, at, grid, named)? {
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
    named: &mut BTreeMap<NodeId, Hash>,
) -> Result<Argued, EngineError> {
    let Some(form) = substituted_closed_form(typing, arg) else {
        return identity_in(typing, arg, named).map(Argued::Node);
    };
    let body = before(&form.body, at, grid);
    Ok(match crate::lower::constant_value(&body, form.var) {
        Some(v) => Argued::Number(v + 0.0),
        None => Argued::Node(formula_identity(&ClosedForm { body, ..form })),
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

fn body_points(f: &Body, grid: Grid, out: &mut BTreeSet<i64>) {
    match f {
        Body::Shift { .. } | Body::Warp { .. } => {}
        Body::Crop { of, l, r, fall, .. } => {
            out.extend(opens(grid, l.value()));
            out.extend(closes(grid, r.value(), *fall));
            body_points(&of.body, grid, out);
        }
        other => {
            for p in sva_formula::closed_form::children(other) {
                body_points(&p.body, grid, out);
            }
        }
    }
}

/// `f` with every crop opening at or past `at` shut, and every one closing there left open.
fn before(f: &Body, at: i64, grid: Grid) -> Body {
    match f {
        Body::Shift { .. } | Body::Warp { .. } => f.clone(),
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
            let inner = Part::new(of.origin, before(&of.body, at, grid));
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
        other => map_children(other, |p| Part::new(p.origin, before(&p.body, at, grid))),
    }
}
