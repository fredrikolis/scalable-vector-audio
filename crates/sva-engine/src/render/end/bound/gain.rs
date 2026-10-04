// Concern: bounds how far a unit change in one node can move a node reading it | Non-concern: either node's own magnitude | IO: (reader, read) -> a gain, or none

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash as _, Hasher};

use sva_formula::{ContentHasher, Hash, HashDomain, NodeId, Var};

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
    let path = path(tys, read);
    if !path.contains(&reader) {
        return Some(0.0);
    }
    let on = |n: &NodeId| path.contains(n);
    let below = |id: NodeId| below(tys, &path, id);
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

/// What `gain` is a function of: each node from `read` up to `reader` by its shape and grid over
/// what it reads, each read off that path by its identity and grid, `read` by its grid alone,
/// whatever it holds; `None` where a node refuses one.
pub(in crate::render::end) fn key(tys: &Typing, reader: NodeId, read: NodeId) -> Option<Hash> {
    let path = path(tys, read);
    let gridded = |shape: Hash, id: NodeId| {
        let mut grid = DefaultHasher::new();
        tys.grid(id).hash(&mut grid);
        let mut hasher = ContentHasher::new(HashDomain::GainBoundKey);
        hasher.hash(shape);
        hasher.word(grid.finish());
        hasher.finish()
    };
    if !path.contains(&reader) {
        return Some(gridded(Hash(0, 0), reader));
    }
    let mut keys = BTreeMap::from([(read, gridded(Hash(0, 0), read))]);
    for id in tys.unfolded_over(reader, |id| below(tys, &path, id), |id| id == read) {
        let mut named = |n: NodeId| match keys.get(&n) {
            Some(held) => Ok(*held),
            None => Ok(gridded(crate::refs::identity(tys, n)?, n)),
        };
        let shape = crate::refs::shape(tys, id, &mut named).ok()?;
        keys.insert(id, gridded(shape, id));
    }
    keys.get(&reader).copied()
}

/// `read` and every node reading it.
fn path(tys: &Typing, read: NodeId) -> BTreeSet<NodeId> {
    let mut path = BTreeSet::from([read]);
    let mut open = vec![read];
    while let Some(at) = open.pop() {
        for up in tys.readers_of(at) {
            if path.insert(up) {
                open.push(up);
            }
        }
    }
    path
}

/// What `id` reads on `path`, its moving times included.
fn below(tys: &Typing, path: &BTreeSet<NodeId>, id: NodeId) -> Vec<NodeId> {
    let mut out = tys.operands(id);
    if let Value::Read { at, .. } = tys.value(id) {
        out.extend(at.moving());
    }
    out.retain(|n| path.contains(n));
    out
}

/// Whether `id`'s own renderer reads itself back, as a fold over that renderer.
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
        Value::ClosedForm(_) | Value::Noise(_) | Value::SelfAt { .. } => Vec::new(),
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
