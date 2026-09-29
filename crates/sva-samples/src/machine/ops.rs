// Concern: the postfix op array one node renderer lowers to, and the width each slot holds | Non-concern: lowering into it or running it (mod.rs) | IO: (&NodeRenderer, &Layout) -> Vec<Op> + Vec<usize>

use crate::error::SampleError;
use crate::machine::renderer::{
    Between, Binary, Formula, Grid, Index, Map, NodeRenderer, Site, SiteId, Slot, Stepped, Unary,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    Const(f64),
    Time,
    /// `None` where its constants overflow on the program's grid.
    Wrap(Option<Stepped>),
    /// The draw of `seed` at the step of the rate nearest each sample; `None` where that map
    /// overflows.
    Noise {
        seed: u64,
        at: Option<Map>,
    },
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
    /// The sample at the program's index `at`, over the `arity` step instants below it.
    Indexed {
        slot: Slot,
        at: usize,
        arity: usize,
        reach: Option<(i64, i64)>,
    },
    Instant {
        at: usize,
        arity: usize,
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
        window: (i64, i64),
        a: f64,
        b: f64,
        rise: f64,
        fall: f64,
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

/// What the engine already knows from typing: the grid the node steps on, how wide it is,
/// how wide each collapsed read is, and which call sites the renderer opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub grid: Grid,
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
    pub(crate) indices: Vec<Index<usize>>,
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

fn indexed(
    index: &Index,
    layout: &Layout,
    out: &mut Lowered,
) -> Result<(usize, Vec<usize>), SampleError> {
    let mut widths = Vec::new();
    let program = index.mapped(&mut |time| {
        widths.push(lower(time, layout, out)?);
        Ok::<usize, SampleError>(widths.len() - 1)
    })?;
    out.indices.push(program);
    Ok((out.indices.len() - 1, widths))
}

fn lower(r: &NodeRenderer, layout: &Layout, out: &mut Lowered) -> Result<usize, SampleError> {
    if let NodeRenderer::Mul(parts) = r
        && let Some((slot, at, by)) = scaled_read(parts)
    {
        return Ok(out.push(Op::ReadScaled { slot, at, by }, slot_width(slot, layout)));
    }
    let (op, operands) = match r {
        NodeRenderer::Formula { formula, time, .. } => {
            let operands = vec![lower(time, layout, out)?];
            out.formulas.push(formula.clone());
            let at = out.formulas.len() - 1;
            (Op::Formula { at }, operands)
        }
        NodeRenderer::Indexed { slot, index, reach } => {
            let (at, operands) = indexed(index, layout, out)?;
            let op = Op::Indexed {
                slot: *slot,
                at,
                arity: operands.len(),
                reach: *reach,
            };
            (op, operands)
        }
        NodeRenderer::Instant(index) => {
            let (at, operands) = indexed(index, layout, out)?;
            let arity = operands.len();
            (Op::Instant { at, arity }, operands)
        }
        other => {
            let operands = other
                .operands()
                .into_iter()
                .map(|p| lower(p, layout, out))
                .collect::<Result<Vec<_>, _>>()?;
            (op_of(other, layout), operands)
        }
    };
    let w = width(r, &operands, layout)?;
    Ok(out.push(op, w))
}

fn op_of(r: &NodeRenderer, layout: &Layout) -> Op {
    match r {
        NodeRenderer::Const(v) => Op::Const(*v),
        NodeRenderer::Time => Op::Time,
        NodeRenderer::Wrap(wrap) => Op::Wrap(wrap.on(layout.grid)),
        NodeRenderer::Noise(seed) => Op::Noise {
            seed: *seed,
            at: Map::rounded(layout.grid.a, 0, layout.grid.d, Between::Even),
        },
        NodeRenderer::Read { slot, map } => Op::Read {
            slot: *slot,
            at: *map,
        },
        NodeRenderer::Add(parts) => Op::Add(parts.len()),
        NodeRenderer::Mul(parts) => Op::Mul(parts.len()),
        NodeRenderer::Join(parts) => Op::Join(parts.len()),
        NodeRenderer::Sub(..) => Op::Sub,
        NodeRenderer::Div(..) => Op::Div,
        NodeRenderer::Pow(..) => Op::Pow,
        NodeRenderer::Map(f, _) => Op::Map(*f),
        NodeRenderer::Zip(f, ..) => Op::Zip(*f),
        NodeRenderer::Crop {
            window,
            a,
            b,
            rise,
            fall,
            ..
        } => Op::Crop {
            window: *window,
            a: *a,
            b: *b,
            rise: *rise,
            fall: *fall,
        },
        NodeRenderer::Channel { k, .. } => Op::Channel(*k),
        NodeRenderer::Filter { site, from, .. } => Op::Filter {
            site: *site,
            from: *from,
        },
        NodeRenderer::Physics { site, from, args } => Op::Physics {
            site: *site,
            from: *from,
            arity: args.len(),
        },
        NodeRenderer::Formula { .. } | NodeRenderer::Indexed { .. } | NodeRenderer::Instant(_) => {
            unreachable!("lowered with the program tables they name")
        }
    }
}

/// The width `r` holds over operands `operands` wide, in `NodeRenderer::operands` order.
pub(crate) fn width(
    r: &NodeRenderer,
    operands: &[usize],
    layout: &Layout,
) -> Result<usize, SampleError> {
    let met = |from: usize| operands.iter().try_fold(from, |w, &o| meet(w, o));
    Ok(match r {
        NodeRenderer::Const(_)
        | NodeRenderer::Time
        | NodeRenderer::Wrap(_)
        | NodeRenderer::Noise(_) => 1,
        NodeRenderer::Read { slot, .. } => slot_width(*slot, layout),
        NodeRenderer::Formula { width, .. } => match operands {
            [times] if *times != 1 && times != width => {
                return Err(SampleError::WidthMismatch {
                    left: *times,
                    right: *width,
                });
            }
            _ => *width,
        },
        NodeRenderer::Indexed { slot, .. } => slot_width(*slot, layout),
        NodeRenderer::Instant(_) | NodeRenderer::Physics { .. } => 1,
        NodeRenderer::Add(_) | NodeRenderer::Mul(_) => met(1)?,
        NodeRenderer::Sub(..)
        | NodeRenderer::Div(..)
        | NodeRenderer::Pow(..)
        | NodeRenderer::Zip(..)
        | NodeRenderer::Filter { .. } => met(operands[0])?,
        NodeRenderer::Map(..) | NodeRenderer::Crop { .. } => operands[0],
        NodeRenderer::Join(_) => operands.iter().sum(),
        NodeRenderer::Channel { k, .. } => match operands[0] {
            w if *k >= w => return Err(SampleError::ChannelOutOfRange { k: *k, width: w }),
            _ => 1,
        },
    })
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
