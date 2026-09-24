// Concern: the postfix op array one node renderer lowers to, and the width each slot holds | Non-concern: lowering into it or running it (mod.rs) | IO: (&NodeRenderer, &Layout) -> Vec<Op> + Vec<usize>

use crate::error::SampleError;
use crate::machine::renderer::{Binary, BufId, NodeRenderer, Site, SiteId, Unary};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    Const(f64),
    Time,
    Read {
        id: BufId,
        shift: i64,
    },
    SelfAt {
        steps: u32,
    },
    Add(usize),
    Mul(usize),
    Sub,
    Div,
    Pow,
    Map(Unary),
    Zip(Binary),
    Crop {
        a: f64,
        b: f64,
        rise: f64,
        fall: f64,
    },
    Join(usize),
    Channel(usize),
    Filter(SiteId),
    Physics(SiteId),
}

/// What the engine already knows from typing: how wide the node is, how wide each collapsed
/// read is, and which call sites the renderer opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width: usize,
    pub read_widths: Vec<usize>,
    pub sites: Vec<Site>,
}

/// Equal widths pass, a mono side widens, anything else refuses.
fn meet(a: usize, b: usize) -> Result<usize, SampleError> {
    match (a, b) {
        (a, b) if a == b => Ok(a),
        (1, b) => Ok(b),
        (a, 1) => Ok(a),
        (left, right) => Err(SampleError::WidthMismatch { left, right }),
    }
}

/// Postfix, so the runner needs no recursion and every operand's width is already settled
/// by the time the op that consumes it is reached.
pub(crate) fn lower(
    renderer: &NodeRenderer,
    layout: &Layout,
    ops: &mut Vec<Op>,
    widths: &mut Vec<usize>,
) -> Result<usize, SampleError> {
    let push = |op: Op, width: usize, ops: &mut Vec<Op>, widths: &mut Vec<usize>| {
        ops.push(op);
        widths.push(width);
        width
    };
    let w = match renderer {
        NodeRenderer::Const(v) => push(Op::Const(*v), 1, ops, widths),
        NodeRenderer::Time => push(Op::Time, 1, ops, widths),
        NodeRenderer::Buffer { id, shift } => {
            let width = layout.read_widths[id.0 as usize];
            push(
                Op::Read {
                    id: *id,
                    shift: *shift,
                },
                width,
                ops,
                widths,
            )
        }
        NodeRenderer::SelfAt { steps } => {
            push(Op::SelfAt { steps: *steps }, layout.width, ops, widths)
        }
        NodeRenderer::Add(parts) | NodeRenderer::Mul(parts) => {
            let mut width = 1;
            for p in parts {
                width = meet(width, lower(p, layout, ops, widths)?)?;
            }
            let op = match renderer {
                NodeRenderer::Add(_) => Op::Add(parts.len()),
                _ => Op::Mul(parts.len()),
            };
            push(op, width, ops, widths)
        }
        NodeRenderer::Sub(a, b) | NodeRenderer::Div(a, b) | NodeRenderer::Pow(a, b) => {
            let wa = lower(a, layout, ops, widths)?;
            let wb = lower(b, layout, ops, widths)?;
            let op = match renderer {
                NodeRenderer::Sub(..) => Op::Sub,
                NodeRenderer::Div(..) => Op::Div,
                _ => Op::Pow,
            };
            push(op, meet(wa, wb)?, ops, widths)
        }
        NodeRenderer::Map(f, x) => {
            let w = lower(x, layout, ops, widths)?;
            push(Op::Map(*f), w, ops, widths)
        }
        NodeRenderer::Zip(f, a, b) => {
            let wa = lower(a, layout, ops, widths)?;
            let wb = lower(b, layout, ops, widths)?;
            push(Op::Zip(*f), meet(wa, wb)?, ops, widths)
        }
        NodeRenderer::Crop {
            x,
            a,
            b,
            rise,
            fall,
        } => {
            let w = lower(x, layout, ops, widths)?;
            let crop = Op::Crop {
                a: *a,
                b: *b,
                rise: *rise,
                fall: *fall,
            };
            push(crop, w, ops, widths)
        }
        NodeRenderer::Join(parts) => {
            let mut width = 0;
            for p in parts {
                width += lower(p, layout, ops, widths)?;
            }
            push(Op::Join(parts.len()), width, ops, widths)
        }
        NodeRenderer::Channel { x, k } => {
            let w = lower(x, layout, ops, widths)?;
            if *k >= w {
                return Err(SampleError::ChannelOutOfRange { k: *k, width: w });
            }
            push(Op::Channel(*k), 1, ops, widths)
        }
        NodeRenderer::Filter {
            site,
            x,
            cutoff,
            q,
            gain,
        } => {
            let mut width = lower(x, layout, ops, widths)?;
            for arg in [cutoff, q, gain] {
                width = meet(width, lower(arg, layout, ops, widths)?)?;
            }
            push(Op::Filter(*site), width, ops, widths)
        }
        NodeRenderer::Physics { site } => push(Op::Physics(*site), 1, ops, widths),
    };
    Ok(w)
}
