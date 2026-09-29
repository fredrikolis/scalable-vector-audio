// Concern: the postfix op array one node renderer lowers to, and the width each slot holds | Non-concern: lowering into it or running it (mod.rs) | IO: (&NodeRenderer, &Layout) -> Vec<Op> + Vec<usize>

use crate::error::SampleError;
use crate::machine::renderer::{
    Binary, Formula, Map, NodeRenderer, Round, Site, SiteId, Slot, Unary, Wrap,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    Const(f64),
    Time,
    Wrap(Wrap),
    Noise(u64),
    Read {
        slot: Slot,
        at: Map,
    },
    /// A read times a constant, the bits of `Mul` over the two.
    ReadScaled {
        slot: Slot,
        at: Map,
        by: f64,
    },
    /// The sample nearest the instant the operand below it names.
    Nearest {
        slot: Slot,
        round: Round,
        plus: i64,
    },
    /// The program's formula `at`, at the instant the operand below it names.
    Formula {
        at: usize,
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
    /// Ahead of a crop whose operand holds no state: where the window is shut, the `over` ops
    /// up to the crop are skipped and the crop writes zero.
    Guard {
        a: f64,
        b: f64,
        rise: f64,
        fall: f64,
        over: usize,
    },
    Join(usize),
    Channel(usize),
    Filter {
        site: SiteId,
        from: i64,
    },
    Physics {
        site: SiteId,
        from: i64,
        arity: usize,
    },
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

/// The ops, one width per op, and the formulas a `Formula` op names by position.
#[derive(Default)]
pub(crate) struct Lowered {
    pub(crate) ops: Vec<Op>,
    pub(crate) widths: Vec<usize>,
    pub(crate) formulas: Vec<Formula>,
}

/// Postfix, so the runner needs no recursion and every operand's width is already settled
/// by the time the op that consumes it is reached.
pub(crate) fn lowered(
    renderer: &NodeRenderer,
    layout: &Layout,
) -> Result<(Lowered, usize), SampleError> {
    let mut out = Lowered::default();
    let width = lower(renderer, layout, &mut out)?;
    Ok((out, width))
}

impl Lowered {
    fn push(&mut self, op: Op, width: usize) -> usize {
        self.ops.push(op);
        self.widths.push(width);
        width
    }
}

fn lower(r: &NodeRenderer, layout: &Layout, out: &mut Lowered) -> Result<usize, SampleError> {
    let w = match r {
        NodeRenderer::Const(v) => out.push(Op::Const(*v), 1),
        NodeRenderer::Time => out.push(Op::Time, 1),
        NodeRenderer::Wrap(wrap) => out.push(Op::Wrap(*wrap), 1),
        NodeRenderer::Noise(seed) => out.push(Op::Noise(*seed), 1),
        NodeRenderer::Read { slot, map } => out.push(
            Op::Read {
                slot: *slot,
                at: *map,
            },
            slot_width(*slot, layout),
        ),
        NodeRenderer::Formula {
            formula,
            width,
            time,
        } => {
            meet(1, lower(time, layout, out)?)?;
            out.formulas.push(formula.clone());
            let at = out.formulas.len() - 1;
            out.push(Op::Formula { at }, *width)
        }
        NodeRenderer::Nearest {
            slot,
            time,
            round,
            plus,
            ..
        } => {
            meet(1, lower(time, layout, out)?)?;
            let op = Op::Nearest {
                slot: *slot,
                round: *round,
                plus: *plus,
            };
            out.push(op, slot_width(*slot, layout))
        }
        NodeRenderer::Mul(parts) if let Some((slot, at, by)) = scaled_read(parts) => {
            out.push(Op::ReadScaled { slot, at, by }, slot_width(slot, layout))
        }
        NodeRenderer::Add(parts) | NodeRenderer::Mul(parts) => {
            let mut width = 1;
            for p in parts {
                width = meet(width, lower(p, layout, out)?)?;
            }
            let op = match r {
                NodeRenderer::Add(_) => Op::Add(parts.len()),
                _ => Op::Mul(parts.len()),
            };
            out.push(op, width)
        }
        NodeRenderer::Sub(a, b) | NodeRenderer::Div(a, b) | NodeRenderer::Pow(a, b) => {
            let wa = lower(a, layout, out)?;
            let wb = lower(b, layout, out)?;
            let op = match r {
                NodeRenderer::Sub(..) => Op::Sub,
                NodeRenderer::Div(..) => Op::Div,
                _ => Op::Pow,
            };
            out.push(op, meet(wa, wb)?)
        }
        NodeRenderer::Map(f, x) => {
            let w = lower(x, layout, out)?;
            out.push(Op::Map(*f), w)
        }
        NodeRenderer::Zip(f, a, b) => {
            let wa = lower(a, layout, out)?;
            let wb = lower(b, layout, out)?;
            out.push(Op::Zip(*f), meet(wa, wb)?)
        }
        NodeRenderer::Crop {
            x,
            a,
            b,
            rise,
            fall,
        } => {
            let guarded = x.stateless();
            let guard = out.ops.len();
            if guarded {
                out.push(Op::Const(0.0), 1);
            }
            let w = lower(x, layout, out)?;
            if guarded {
                out.ops[guard] = Op::Guard {
                    a: *a,
                    b: *b,
                    rise: *rise,
                    fall: *fall,
                    over: out.ops.len() - guard,
                };
            }
            let crop = Op::Crop {
                a: *a,
                b: *b,
                rise: *rise,
                fall: *fall,
            };
            out.push(crop, w)
        }
        NodeRenderer::Join(parts) => {
            let mut width = 0;
            for p in parts {
                width += lower(p, layout, out)?;
            }
            out.push(Op::Join(parts.len()), width)
        }
        NodeRenderer::Channel { x, k } => {
            let w = lower(x, layout, out)?;
            if *k >= w {
                return Err(SampleError::ChannelOutOfRange { k: *k, width: w });
            }
            out.push(Op::Channel(*k), 1)
        }
        NodeRenderer::Filter {
            site,
            from,
            x,
            cutoff,
            q,
            gain,
        } => {
            let mut width = lower(x, layout, out)?;
            for arg in [cutoff, q, gain] {
                width = meet(width, lower(arg, layout, out)?)?;
            }
            let op = Op::Filter {
                site: *site,
                from: *from,
            };
            out.push(op, width)
        }
        NodeRenderer::Physics { site, from, args } => {
            for arg in args {
                meet(1, lower(arg, layout, out)?)?;
            }
            let op = Op::Physics {
                site: *site,
                from: *from,
                arity: args.len(),
            };
            out.push(op, 1)
        }
    };
    Ok(w)
}

fn slot_width(slot: Slot, layout: &Layout) -> usize {
    match slot {
        Slot::Read(id) => layout.read_widths[id.0 as usize],
        Slot::Own => layout.width,
    }
}

fn scaled_read(parts: &[NodeRenderer]) -> Option<(Slot, Map, f64)> {
    match parts {
        [NodeRenderer::Read { slot, map }, NodeRenderer::Const(by)]
        | [NodeRenderer::Const(by), NodeRenderer::Read { slot, map }] => Some((*slot, *map, *by)),
        _ => None,
    }
}
