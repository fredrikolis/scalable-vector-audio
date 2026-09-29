// Concern: runs one renderer span by span, each span without the reads that answer zero there | Non-concern: where a read is zero (the caller's windows) | IO: (NodeRenderer, Ctx, live windows) -> Buffer

use super::ops::{Layout, lowered};
use super::renderer::{BufId, Map, NodeRenderer, Slot};
use super::tape::Tape;
use super::{Ctx, Machine};
use crate::buffer::Buffer;
use crate::collapse::Extent;
use crate::error::SampleError;

pub struct Span {
    pub from: i64,
    pub to: i64,
    pub renderer: NodeRenderer,
}

impl NodeRenderer {
    /// Outside `live[k]` read `k` answers exactly +0.0, which a sum starting from +0 drops
    /// without a bit changing. Every call site's state and the node's own past run on
    /// across the spans.
    pub fn run_live(
        &self,
        layout: &Layout,
        ctx: &Ctx,
        live: &[Extent],
    ) -> Result<Buffer, SampleError> {
        let end = ctx.start + ctx.len as i64;
        let mut machine = Machine::live(self, layout, ctx.grid, (ctx.start, end), live)?;
        let mut own = Tape::new(machine.width(), ctx.len, ctx.start);
        machine.steps(end, ctx.reads, &mut own)?;
        let mut out = Buffer::of_planes(ctx.grid.rate, own.into_planes());
        out.start = ctx.start;
        Ok(out)
    }

    /// `[from, to)` cut where the set of dead reads changes.
    pub fn spans(
        &self,
        layout: &Layout,
        (from, to): (i64, i64),
        live: &[Extent],
    ) -> Result<Vec<Span>, SampleError> {
        let mut leaves = Vec::new();
        buffers(self, &mut leaves);
        let reach = |(id, at): Leaf| at.preimage(live[id.0 as usize]);
        let mut edges = vec![from, to];
        for leaf in &leaves {
            let held = reach(*leaf);
            edges.extend(
                [held.start, held.end]
                    .into_iter()
                    .filter(|e| from < *e && *e < to),
            );
        }
        edges.sort_unstable();
        edges.dedup();
        let mut out: Vec<Span> = Vec::new();
        for pair in edges.windows(2) {
            let span = Extent::new(pair[0], pair[1]);
            let dead = |leaf: Leaf| reach(leaf).intersect(span).is_empty();
            let renderer = pruned(self, &dead, layout)?;
            match out.last_mut() {
                Some(last) if last.renderer == renderer => last.to = span.end,
                _ => out.push(Span {
                    from: span.start,
                    to: span.end,
                    renderer,
                }),
            }
        }
        Ok(out)
    }

    /// One per op, and a formula's own.
    pub fn ops(&self, layout: &Layout) -> Result<usize, SampleError> {
        let (lowered, _) = lowered(self, layout)?;
        let formulas: usize = lowered.formulas.iter().map(|f| f.ops()).sum();
        Ok(lowered.ops.len() + formulas)
    }
}

fn width(r: &NodeRenderer, layout: &Layout) -> Result<usize, SampleError> {
    Ok(lowered(r, layout)?.1)
}

/// A buffer read at a fixed map.
type Leaf = (BufId, Map);

fn buffers(r: &NodeRenderer, out: &mut Vec<Leaf>) {
    if let NodeRenderer::Read {
        slot: Slot::Read(id),
        map,
    } = r
    {
        out.push((*id, *map));
    }
    for part in operands(r) {
        buffers(part, out);
    }
}

/// Every node over its pruned operands, then each sum without the terms that are exact zero
/// where that keeps its width.
fn pruned(
    r: &NodeRenderer,
    dead: &dyn Fn(Leaf) -> bool,
    layout: &Layout,
) -> Result<NodeRenderer, SampleError> {
    let whole = width(r, layout)?;
    let held = rebuilt(r, &mut |p| pruned(p, dead, layout))?;
    let zero = |p: &NodeRenderer| zero(p, dead);
    Ok(match held {
        NodeRenderer::Add(parts) => {
            let live: Vec<NodeRenderer> = parts.iter().filter(|p| !zero(p)).cloned().collect();
            match live.is_empty() {
                true if whole == 1 => NodeRenderer::Const(0.0),
                false if width(&NodeRenderer::Add(live.clone()), layout)? == whole => {
                    NodeRenderer::Add(live)
                }
                _ => NodeRenderer::Add(parts),
            }
        }
        NodeRenderer::Sub(a, b) if zero(&b) && width(&a, layout)? == whole => *a,
        other => other,
    })
}

/// Exactly +0.0 at every sample of the span.
fn zero(r: &NodeRenderer, dead: &dyn Fn(Leaf) -> bool) -> bool {
    match r {
        NodeRenderer::Read {
            slot: Slot::Read(id),
            map,
        } => dead((*id, *map)),
        NodeRenderer::Const(v) => v.to_bits() == 0,
        NodeRenderer::Crop { x, .. } => zero(x, dead),
        NodeRenderer::Add(parts) => parts.iter().all(|p| zero(p, dead)),
        NodeRenderer::Sub(a, b) => zero(a, dead) && zero(b, dead),
        _ => false,
    }
}

fn operands(r: &NodeRenderer) -> Vec<&NodeRenderer> {
    match r {
        NodeRenderer::Add(set) | NodeRenderer::Mul(set) | NodeRenderer::Join(set) => {
            set.iter().collect()
        }
        NodeRenderer::Sub(a, b)
        | NodeRenderer::Div(a, b)
        | NodeRenderer::Pow(a, b)
        | NodeRenderer::Zip(_, a, b) => vec![a, b],
        NodeRenderer::Map(_, x)
        | NodeRenderer::Crop { x, .. }
        | NodeRenderer::Channel { x, .. } => vec![x],
        NodeRenderer::Filter {
            x, cutoff, q, gain, ..
        } => vec![x, cutoff, q, gain],
        NodeRenderer::Physics { args, .. } => args.iter().collect(),
        NodeRenderer::Formula { time, .. } | NodeRenderer::Nearest { time, .. } => vec![time],
        _ => Vec::new(),
    }
}

type Each<'a> = dyn FnMut(&NodeRenderer) -> Result<NodeRenderer, SampleError> + 'a;

fn rebuilt(r: &NodeRenderer, each: &mut Each) -> Result<NodeRenderer, SampleError> {
    let mut one = |p: &NodeRenderer| each(p).map(Box::new);
    Ok(match r {
        NodeRenderer::Add(set) => {
            NodeRenderer::Add(set.iter().map(&mut *each).collect::<Result<_, _>>()?)
        }
        NodeRenderer::Mul(set) => {
            NodeRenderer::Mul(set.iter().map(&mut *each).collect::<Result<_, _>>()?)
        }
        NodeRenderer::Join(set) => {
            NodeRenderer::Join(set.iter().map(&mut *each).collect::<Result<_, _>>()?)
        }
        NodeRenderer::Sub(a, b) => NodeRenderer::Sub(one(a)?, one(b)?),
        NodeRenderer::Div(a, b) => NodeRenderer::Div(one(a)?, one(b)?),
        NodeRenderer::Pow(a, b) => NodeRenderer::Pow(one(a)?, one(b)?),
        NodeRenderer::Zip(f, a, b) => NodeRenderer::Zip(*f, one(a)?, one(b)?),
        NodeRenderer::Map(f, x) => NodeRenderer::Map(*f, one(x)?),
        NodeRenderer::Channel { x, k } => NodeRenderer::Channel { x: one(x)?, k: *k },
        NodeRenderer::Crop {
            x,
            a,
            b,
            rise,
            fall,
        } => NodeRenderer::Crop {
            x: one(x)?,
            a: *a,
            b: *b,
            rise: *rise,
            fall: *fall,
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
            x: one(x)?,
            cutoff: one(cutoff)?,
            q: one(q)?,
            gain: one(gain)?,
        },
        NodeRenderer::Physics { site, from, args } => NodeRenderer::Physics {
            site: *site,
            from: *from,
            args: args.iter().map(&mut *each).collect::<Result<_, _>>()?,
        },
        NodeRenderer::Formula {
            formula,
            width,
            time,
        } => NodeRenderer::Formula {
            formula: formula.clone(),
            width: *width,
            time: one(time)?,
        },
        NodeRenderer::Nearest {
            slot,
            time,
            round,
            plus,
            reach,
        } => NodeRenderer::Nearest {
            slot: *slot,
            time: one(time)?,
            round: *round,
            plus: *plus,
            reach: *reach,
        },
        leaf => leaf.clone(),
    })
}
