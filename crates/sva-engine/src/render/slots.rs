// Concern: what each buffer a sampled node reads contributed to it | Non-concern: building the program (sampled.rs), sharing a target's energy out (sva-samples) | IO: (NodeId) -> a ref and its addend

use sva_formula::NodeId;
use sva_samples::{BufId, Buffer, NodeRenderer};

use crate::cast::Cast;
use crate::error::EngineError;
use crate::render::Render;
use crate::render::sampled::{self, Program};
use crate::typing::Value;

/// Every ref a sampled node reads, one row per name however many slots carry it.
pub(super) fn refs_read(render: &Render, node: NodeId) -> Result<Vec<NodeId>, EngineError> {
    let Some(program) = program(render, node)? else {
        return Ok(Vec::new());
    };
    let here = render.tys.name(node);
    let mut out: Vec<NodeId> = Vec::new();
    for source in program.reads {
        let Some(ref_node) = behind(render, here, source) else {
            continue;
        };
        let name = render.tys.name(ref_node);
        if !out.iter().any(|held| render.tys.name(*held) == name) {
            out.push(ref_node);
        }
    }
    Ok(out)
}

/// An `istft` reads frames, not slots; every other sampled node built this program once
/// already, so a refusal here is a fault.
fn program(render: &Render, node: NodeId) -> Result<Option<Program>, EngineError> {
    match render.tys.value(node) {
        Value::Cast(Cast::Istft, _) => Ok(None),
        _ => sampled::program(render, node).map(Some),
    }
}

/// The ref one slot stands for: its source, or — where that source is a subterm written here,
/// and so carries this node's name — the one ref it reads. A subterm summing several isolates
/// none, and one this render never held has no row.
fn behind(render: &Render, here: &str, source: NodeId) -> Option<NodeId> {
    let held = |id: NodeId| render.buffers.contains_key(&id).then_some(id);
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

/// What one ref put into the sampled node reading it: that node's program run with every
/// other slot silenced, so the refs of a sum add back to it. `None` is an edge no slot
/// isolates, never a run that refused.
pub(super) fn contributed(
    render: &Render,
    parent: NodeId,
    child: NodeId,
) -> Result<Option<Buffer>, EngineError> {
    let Some(program) = program(render, parent)? else {
        return Ok(None);
    };
    let (here, name) = (render.tys.name(parent), render.tys.name(child));
    let kept: Vec<BufId> = program
        .reads
        .iter()
        .enumerate()
        .filter(|(_, source)| {
            behind(render, here, **source).is_some_and(|id| render.tys.name(id) == name)
        })
        .map(|(slot, _)| BufId(slot as u32))
        .collect();
    let held = |id: BufId| kept.contains(&id);
    if kept.is_empty() || !separable(&program.renderer, &held) {
        return Ok(None);
    }
    program
        .writes(render, parent, &silenced(&program.renderer, &held))
        .map(Some)
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
        | NodeRenderer::Buffer { .. }
        | NodeRenderer::SelfAt { .. }
        | NodeRenderer::Physics { .. } => true,
        other => operands(other).iter().any(|p| moves(p)),
    }
}

fn reads(r: &NodeRenderer, kept: &dyn Fn(BufId) -> bool) -> bool {
    match r {
        NodeRenderer::Buffer { id, .. } => kept(*id),
        other => operands(other).iter().any(|p| reads(p, kept)),
    }
}

fn silenced(r: &NodeRenderer, kept: &dyn Fn(BufId) -> bool) -> NodeRenderer {
    match r {
        NodeRenderer::Buffer { id, .. } if !kept(*id) => NodeRenderer::Const(0.0),
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
            a,
            b,
            rise,
            fall,
        } => NodeRenderer::Crop {
            x: one(x, kept),
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
            x,
            cutoff,
            q,
            gain,
        } => NodeRenderer::Filter {
            site: *site,
            x: one(x, kept),
            cutoff: one(cutoff, kept),
            q: one(q, kept),
            gain: one(gain, kept),
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
        _ => Vec::new(),
    }
}
