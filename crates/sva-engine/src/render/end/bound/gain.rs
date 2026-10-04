// Concern: bounds how far a unit change in one node can move a node reading it | Non-concern: either node's own magnitude | IO: (reader, read) -> a gain, or none

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{NodeId, Var};

use crate::cast::Cast;
use crate::lower::number_of;
use crate::time::Q;
use crate::typing::{Typing, Value, When};

use super::filter;
use super::range::{OP, Range};
use super::sampled;

/// Bounds `sup|reader' - reader|` where `sup|read' - read| <= 1`, folded over only the nodes
/// reading `read`; `None` through a loop, a moving parameter, a read that interpolates or a
/// product of two changed values.
pub(in crate::render::end) fn gain(tys: &Typing, reader: NodeId, read: NodeId) -> Option<f64> {
    let mut path = BTreeSet::from([read]);
    let mut open = vec![read];
    while let Some(at) = open.pop() {
        for up in tys.readers_of(at) {
            if path.insert(up) {
                open.push(up);
            }
        }
    }
    if !path.contains(&reader) {
        return Some(0.0);
    }
    let on = |n: &NodeId| path.contains(n);
    let below = |id: NodeId| {
        let mut out = tys.operands(id);
        if let Value::Read { at, .. } = tys.value(id) {
            out.extend(at.moving());
        }
        out.retain(on);
        out
    };
    let mut gains = BTreeMap::from([(read, Some(1.0))]);
    let mut looped = BTreeMap::new();
    for id in tys.unfolded_over(reader, below, |id| id == read) {
        let found = match loops(tys, id, &mut looped) {
            true => None,
            false => moved(tys, id, &|n| match on(&n) {
                true => gains.get(&n).copied().flatten(),
                false => Some(0.0),
            }),
        };
        gains.insert(id, found);
    }
    gains.get(&reader).copied().flatten()
}

/// Whether `id`'s own program reads itself back, as a fold over that program.
fn loops(tys: &Typing, id: NodeId, held: &mut BTreeMap<NodeId, bool>) -> bool {
    for n in tys.unfolded_over(id, |n| program(tys, n), |n| held.contains_key(&n)) {
        let found = match tys.value(n) {
            Value::SelfAt { .. } => true,
            _ => program(tys, n).iter().any(|o| held.get(o) == Some(&true)),
        };
        held.insert(n, found);
    }
    held[&id]
}

fn program(tys: &Typing, id: NodeId) -> Vec<NodeId> {
    match tys.value(id) {
        Value::Cast(Cast::Sample, _) | Value::Read { .. } => Vec::new(),
        Value::ClosedForm(_) | Value::Noise(_) | Value::Stored(_) | Value::SelfAt { .. } => {
            Vec::new()
        }
        _ => tys.operands(id),
    }
}

fn moved(tys: &Typing, id: NodeId, gain: &dyn Fn(NodeId) -> Option<f64>) -> Option<f64> {
    match tys.value(id) {
        Value::Cast(Cast::Sample, source) => gain(*source),
        Value::Read { source, at, .. } => {
            let same = tys.grid(*source) == tys.grid(id);
            match at {
                When::At(map) if map.scale == Q::ONE && same => gain(*source),
                When::Index(_) if same => gain(*source),
                _ => None,
            }
        }
        Value::ClosedForm(form) if form.var == Var::T => Range::of(&form.body).ok()?.moved(gain),
        Value::Op { name, args } => Range::of(&sampled(tys, name, args)?).ok()?.moved(gain),
        Value::Filter {
            shape,
            x,
            cutoff,
            q,
            gain: boost,
        } => {
            let grid = tys.grid(id);
            let params = [*cutoff, *q, *boost];
            if tys.grid(*x) != grid || params.iter().any(|p| gain(*p) != Some(0.0)) {
                return None;
            }
            let [Some(f), Some(q), Some(db)] = params.map(|p| number_of(tys, p)) else {
                return None;
            };
            let (coeffs, _) = sva_samples::filters::coefficients(*shape, f, q, db, grid.sr());
            Some(gain(*x)? * filter::moved(&coeffs)? * (1.0 + OP))
        }
        _ => None,
    }
}
