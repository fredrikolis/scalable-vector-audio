// Concern: what each value a node's program reads contributed to it | Non-concern: building the program (table/), sharing a target's energy out (sva-samples) | IO: (NodeId) -> a ref and its addend

use sva_formula::NodeId;
use sva_samples::{BufId, Buffer, NodeRenderer, Slot};

use crate::error::EngineError;
use crate::render::Render;
use crate::render::table::{Kind, Table};

/// Every ref a node's program reads, one row per name however many slots carry it.
pub(super) fn refs_read(
    render: &Render,
    node: NodeId,
    holds: &dyn Fn(NodeId) -> bool,
) -> Result<Vec<NodeId>, EngineError> {
    let Some((table, at)) = program(render, node) else {
        return Ok(Vec::new());
    };
    let here = render.tys.name(node);
    let mut out: Vec<NodeId> = Vec::new();
    for read in &table.values[at].reads {
        let Some(source) = table.values[*read].node else {
            continue;
        };
        let Some(ref_node) = behind(render, here, source, holds) else {
            continue;
        };
        let name = render.tys.name(ref_node);
        if !out.iter().any(|held| render.tys.name(*held) == name) {
            out.push(ref_node);
        }
    }
    Ok(out)
}

/// The table's program for `node`, where it computes one.
fn program(render: &Render, node: NodeId) -> Option<(&Table, usize)> {
    let table = render.table.as_ref()?;
    let at = table.of(node)?;
    matches!(table.values[at].kind, Kind::Program(_)).then_some((table, at))
}

pub(super) fn reads_held(render: &Render, node: NodeId) -> Result<bool, EngineError> {
    Ok(render.table.as_ref().is_some_and(|t| t.of(node).is_some()))
}

/// The ref one slot stands for: its source, or — where that source is a subterm written here,
/// and so carries this node's name — the one ref it reads. A subterm summing several isolates
/// none, and one this render never held has no row.
fn behind(
    render: &Render,
    here: &str,
    source: NodeId,
    holds: &dyn Fn(NodeId) -> bool,
) -> Option<NodeId> {
    let held = |id: NodeId| holds(id).then_some(id);
    if render.tys.name(source) != here {
        return held(source);
    }
    let read = crate::schedule::read_operands(&render.tys, source);
    let mut names: Vec<&str> = read.iter().map(|id| render.tys.name(*id)).collect();
    names.dedup();
    match (names.len(), read.first()) {
        (1, Some(only)) if render.tys.name(*only) != here => held(*only),
        _ => None,
    }
}

/// What one ref put into the node reading it: that node's program run with every other slot
/// silenced, so the refs of a sum add back to it. `None` is an edge no slot isolates, never a
/// run that refused.
pub(super) fn contributed(
    render: &Render,
    parent: NodeId,
    child: NodeId,
) -> Result<Option<Buffer>, EngineError> {
    let holds = |id: NodeId| render.buffers.contains_key(&id);
    let Some((renderer, kept)) = isolated(render, parent, child, &holds)? else {
        return Ok(None);
    };
    let (table, at) = program(render, parent).expect("an isolated edge is a program's");
    let range = render.range.expect("a ledger reads a decided range");
    let held = |id: BufId| kept.contains(&id);
    table
        .rerun(at, &silenced(&renderer, &held), range)
        .map(Some)
}

pub(super) fn isolated(
    render: &Render,
    parent: NodeId,
    child: NodeId,
    holds: &dyn Fn(NodeId) -> bool,
) -> Result<Option<(NodeRenderer, Vec<BufId>)>, EngineError> {
    let Some((table, at)) = program(render, parent) else {
        return Ok(None);
    };
    let Kind::Program(program) = &table.values[at].kind else {
        return Ok(None);
    };
    let (here, name) = (render.tys.name(parent), render.tys.name(child));
    let kept: Vec<BufId> = table.values[at]
        .reads
        .iter()
        .enumerate()
        .filter(|(_, read)| {
            table.values[**read]
                .node
                .and_then(|source| behind(render, here, source, holds))
                .is_some_and(|id| render.tys.name(id) == name)
        })
        .map(|(slot, _)| BufId(slot as u32))
        .collect();
    let held = |id: BufId| kept.contains(&id);
    match kept.is_empty() || !separable(&program.renderer, &held) {
        true => Ok(None),
        false => Ok(Some((program.renderer.clone(), kept))),
    }
}

/// Two moving operands under one product have no addend apiece; a solver moves as a slot
/// does. A filter is linear in what it filters, so only its slots count.
fn separable(r: &NodeRenderer, kept: &dyn Fn(BufId) -> bool) -> bool {
    let parts = operands(r);
    match r {
        NodeRenderer::Add(_) | NodeRenderer::Sub(..) => parts.iter().all(|p| separable(p, kept)),
        _ => {
            let counts = |p: &NodeRenderer| match r {
                NodeRenderer::Filter { .. } => reads(p, &|_| true),
                _ => moves(p),
            };
            let mut holding = parts.iter().filter(|p| counts(p));
            match (holding.next(), holding.next()) {
                (None, _) => true,
                (Some(only), None) => separable(only, kept),
                _ => !reads(r, kept),
            }
        }
    }
}

fn moves(r: &NodeRenderer) -> bool {
    match r {
        NodeRenderer::Const(_) => false,
        NodeRenderer::Time
        | NodeRenderer::Read { .. }
        | NodeRenderer::Indexed { .. }
        | NodeRenderer::Noise(_)
        | NodeRenderer::Physics { .. } => true,
        other => operands(other).iter().any(|p| moves(p)),
    }
}

fn reads(r: &NodeRenderer, kept: &dyn Fn(BufId) -> bool) -> bool {
    match r {
        NodeRenderer::Read {
            slot: Slot::Read(id),
            ..
        }
        | NodeRenderer::Indexed {
            slot: Slot::Read(id),
            ..
        } if kept(*id) => true,
        NodeRenderer::Read { .. } => false,
        other => operands(other).iter().any(|p| reads(p, kept)),
    }
}

fn silenced(r: &NodeRenderer, kept: &dyn Fn(BufId) -> bool) -> NodeRenderer {
    match r {
        NodeRenderer::Read {
            slot: Slot::Read(id),
            ..
        }
        | NodeRenderer::Indexed {
            slot: Slot::Read(id),
            ..
        } if !kept(*id) => NodeRenderer::Const(0.0),
        NodeRenderer::Add(set) => NodeRenderer::Add(each(set, kept)),
        NodeRenderer::Mul(set) => NodeRenderer::Mul(each(set, kept)),
        NodeRenderer::Join(set) => NodeRenderer::Join(each(set, kept)),
        NodeRenderer::Sub(l, r) => NodeRenderer::Sub(one(l, kept), one(r, kept)),
        NodeRenderer::Div(l, r) => NodeRenderer::Div(one(l, kept), one(r, kept)),
        NodeRenderer::Pow(l, r) => NodeRenderer::Pow(one(l, kept), one(r, kept)),
        NodeRenderer::Zip(op, l, r) => NodeRenderer::Zip(*op, one(l, kept), one(r, kept)),
        NodeRenderer::Map(op, x) => NodeRenderer::Map(*op, one(x, kept)),
        NodeRenderer::Crop {
            x,
            window,
            a,
            b,
            rise,
            fall,
        } => NodeRenderer::Crop {
            x: one(x, kept),
            window: *window,
            a: *a,
            b: *b,
            rise: *rise,
            fall: *fall,
        },
        NodeRenderer::Channel { x, k } => NodeRenderer::Channel {
            x: one(x, kept),
            k: *k,
        },
        NodeRenderer::Filter {
            site,
            from,
            x,
            cutoff,
            q,
            gain,
        } => NodeRenderer::Filter {
            site: *site,
            from: *from,
            x: one(x, kept),
            cutoff: one(cutoff, kept),
            q: one(q, kept),
            gain: one(gain, kept),
        },
        NodeRenderer::Physics { site, from, args } => NodeRenderer::Physics {
            site: *site,
            from: *from,
            args: each(args, kept),
        },
        leaf => leaf.clone(),
    }
}

fn each(set: &[NodeRenderer], kept: &dyn Fn(BufId) -> bool) -> Vec<NodeRenderer> {
    set.iter().map(|p| silenced(p, kept)).collect()
}

fn one(r: &NodeRenderer, kept: &dyn Fn(BufId) -> bool) -> Box<NodeRenderer> {
    Box::new(silenced(r, kept))
}

fn operands(r: &NodeRenderer) -> Vec<&NodeRenderer> {
    match r {
        NodeRenderer::Add(set) | NodeRenderer::Mul(set) | NodeRenderer::Join(set) => {
            set.iter().collect()
        }
        NodeRenderer::Sub(l, r)
        | NodeRenderer::Div(l, r)
        | NodeRenderer::Pow(l, r)
        | NodeRenderer::Zip(_, l, r) => vec![l, r],
        NodeRenderer::Map(_, x)
        | NodeRenderer::Crop { x, .. }
        | NodeRenderer::Channel { x, .. } => {
            vec![x]
        }
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => vec![x, cutoff, q, gain],
        NodeRenderer::Physics { args, .. } => args.iter().collect(),
        _ => Vec::new(),
    }
}
