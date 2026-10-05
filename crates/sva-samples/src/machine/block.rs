// Concern: runs a block's ops whole across it, then the recurrent ones' tape | Non-concern: which ops recur, how long a block runs | IO: (CompiledOps, reads, states) -> each slot's samples

use super::kernels::{self, Refused};
use super::ops::Op;
use super::read::Here;
use super::tape::Tape;
use super::{CompiledOps, State};
use crate::error::SampleError;

pub(super) const BLOCK: usize = 128;

const _: () = assert!(
    crate::grid::BLOCK % BLOCK as i64 == 0,
    "a grid block holds whole machine blocks"
);

/// Every slot's samples over one block, component after component within each sample, and
/// each sample's instant.
pub(super) struct BlockScratch {
    values: Vec<f64>,
    offsets: Vec<usize>,
    times: Vec<f64>,
    /// Each slot run sample by sample over this block, its own past read inside it.
    pub(super) recurrent: Vec<bool>,
    tape: Tape,
}

impl BlockScratch {
    pub(super) fn of(widths: &[usize]) -> BlockScratch {
        let mut offsets = Vec::with_capacity(widths.len() + 1);
        let mut end = 0;
        for w in widths {
            offsets.push(end);
            end += w * BLOCK;
        }
        offsets.push(end);
        BlockScratch {
            values: vec![0.0; end],
            offsets,
            times: vec![0.0; BLOCK],
            recurrent: Vec::with_capacity(widths.len()),
            tape: Tape::default(),
        }
    }

    pub(super) fn top(&self, p: &CompiledOps, i: usize) -> &[f64] {
        let slot = p.ops.len() - 1;
        let w = p.widths[slot];
        &self.values[self.offsets[slot] + i * w..][..w]
    }
}

/// Samples `[from, from + len)`: every op the block's recurrence leaves out over the block,
/// then the recurrent ops' tape sample after sample. How many samples hold, and the refusal a
/// sample-at-a-time run meets first: the earliest sample, and within it the earliest op.
pub(super) fn run(
    p: &CompiledOps,
    block: &mut BlockScratch,
    here: &Here,
    states: &mut [State],
    (from, len): (i64, usize),
) -> (usize, Option<SampleError>) {
    for (i, t) in block.times[..len].iter_mut().enumerate() {
        *t = here.grid.instant(from + i as i64);
    }
    if block.tape.recurrent() != block.recurrent.as_slice() {
        block.tape = Tape::of(p, &block.offsets, &block.recurrent);
    }
    let mut first: Option<(usize, usize, SampleError)> = None;
    let held = |first: &Option<(usize, usize, SampleError)>| first.as_ref().map_or(len, |f| f.0);
    for &slot in block.tape.whole() {
        let scratch = (
            block.values.as_mut_slice(),
            &block.offsets[..],
            &block.times[..],
        );
        if let Err((at, e)) = fill(p, slot, scratch, here, states, (from, held(&first))) {
            first = Some((at, slot, e));
        }
    }
    let stop = first.as_ref().map(|(at, op, _)| (*at, *op));
    let scratch = (block.values.as_mut_slice(), &block.times[..len]);
    if let Some(refused) = block.tape.run(p, scratch, here, states, (from, len), stop) {
        first = Some(refused);
    }
    (held(&first), first.map(|(_, _, e)| e))
}

/// Sample `i` of the block, at index `n`, into its components.
type Sample<'a> = dyn FnMut(usize, i64, &mut [f64]) -> Result<(), SampleError> + 'a;

/// `f` at each sample of the block in order, and the first it refuses at.
fn each(out: &mut [f64], (from, w): (i64, usize), f: &mut Sample) -> Result<(), Refused> {
    for (i, sample) in out.chunks_exact_mut(w).enumerate() {
        f(i, from + i as i64, sample).map_err(|e| (i, e))?;
    }
    Ok(())
}

/// The block's first `len` samples of one slot.
fn fill(
    p: &CompiledOps,
    slot: usize,
    (values, offsets, times): (&mut [f64], &[usize], &[f64]),
    here: &Here,
    states: &mut [State],
    (from, len): (i64, usize),
) -> Result<(), Refused> {
    let (op, w) = (&p.ops[slot], p.widths[slot]);
    let (times, args) = (&times[..len], &p.args[slot]);
    let (done, rest) = values.split_at_mut(offsets[slot]);
    let out = &mut rest[..w * len];
    let arg = |k: usize, i: usize| {
        let s = args[k];
        let w = p.widths[s];
        &done[offsets[s] + i * w..][..w]
    };
    let operand = |s: usize| {
        let w = p.widths[s];
        (&done[offsets[s]..][..w * len], w)
    };
    match op {
        Op::Const(v) => {
            out.fill(*v);
            Ok(())
        }
        Op::Time => {
            out.copy_from_slice(times);
            Ok(())
        }
        Op::Wrap(wrap) => each(out, (from, w), &mut |_, n, s| {
            s[0] = wrap
                .and_then(|wrap| wrap.at(n))
                .ok_or(SampleError::UnreadablePosition)?;
            Ok(())
        }),
        Op::Noise { seed, at } => each(out, (from, w), &mut |_, n, s| {
            let step = at.ok_or(SampleError::UnreadablePosition)?.at(n);
            s[0] = sva_formula::draw(*seed, step);
            Ok(())
        }),
        Op::Indexed {
            slot, at, reach, ..
        } => each(out, (from, w), &mut |i, n, s| {
            let index = &p.indices[*at];
            let k = kernels::indexed(index, *reach, (n, here.grid), &|j| arg(*j, i)[0])?;
            here.source(*slot, n).nearest(k, s)
        }),
        Op::Instant { at, .. } => each(out, (from, w), &mut |i, n, s| {
            let index = &p.indices[*at];
            s[0] = here
                .grid
                .instant(kernels::indexed(index, None, (n, here.grid), &|j| {
                    arg(*j, i)[0]
                })?);
            Ok(())
        }),
        Op::Read { slot, at } | Op::ReadScaled { slot, at, .. } => {
            let copied = here.source(*slot, from).copied(*at, from, (out, w));
            if !copied {
                each(out, (from, w), &mut |_, n, s| {
                    here.source(*slot, n).mapped(*at, n, s)
                })?;
            }
            if let Op::ReadScaled { by, .. } = op {
                out.iter_mut().for_each(|v| *v *= by);
            }
            Ok(())
        }
        Op::Formula { at } => {
            let times = operand(args[0]);
            kernels::formula(p, *at, (out, w), times, here.grid, (from, 0))
        }
        Op::Add(_) => {
            kernels::sum(out, w, args.iter().map(|&s| operand(s)));
            Ok(())
        }
        Op::Mul(_) => {
            kernels::product(out, w, args.iter().map(|&s| operand(s)));
            Ok(())
        }
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => {
            kernels::pair(*op, out, w, operand(args[0]), operand(args[1]));
            Ok(())
        }
        Op::Map(f) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                kernels::map(*f, arg(0, i), s);
            }
            Ok(())
        }
        Op::Crop {
            window,
            a,
            b,
            rise,
            fall,
        } => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                let at = (from + i as i64, times[i]);
                kernels::crop(*window, [*a, *b, *rise, *fall], at, arg(0, i), s);
            }
            Ok(())
        }
        Op::Join(_) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                kernels::join((0..args.len()).map(|k| arg(k, i)), s);
            }
            Ok(())
        }
        Op::Channel(k) => {
            for (i, v) in out.iter_mut().enumerate() {
                *v = arg(0, i)[*k];
            }
            Ok(())
        }
        Op::Filter { site, from: begins } => {
            let State::Filter(filter) = &mut states[site.0 as usize] else {
                unreachable!("a filter op names a filter site")
            };
            let sr = here.grid.sr();
            each(out, (from, w), &mut |i, n, s| {
                let operands = [0, 1, 2, 3].map(|k| arg(k, i));
                kernels::filter(filter, (*begins, n, sr), operands, s);
                Ok(())
            })
        }
        Op::Physics {
            site, from: begins, ..
        } => {
            let State::Physics(solver) = &mut states[site.0 as usize] else {
                unreachable!("a physics op names a physics site")
            };
            each(out, (from, w), &mut |i, n, s| {
                let operands = (0..args.len()).map(|k| arg(k, i));
                kernels::physics(solver.as_mut(), (*begins, n), operands, s)
            })
        }
    }
}
