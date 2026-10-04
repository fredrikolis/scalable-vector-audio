// Concern: runs compiled ops over a block, its recurrent ops sample by sample | Non-concern: which ops run where, how long a block runs | IO: (CompiledOps, reads, states) -> each slot's samples

use super::ops::Op;
use super::read::{Fresh, Source};
use super::renderer::Slot;
use super::{CompiledOps, State, part};
use crate::buffer::SampleView;
use crate::error::SampleError;
use crate::grid::Grid;

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
        }
    }

    /// Sample `i` of the program's last slot.
    pub(super) fn top(&self, p: &CompiledOps, i: usize) -> &[f64] {
        let slot = p.ops.len() - 1;
        let w = p.widths[slot];
        &self.values[self.offsets[slot] + i * w..][..w]
    }
}

/// What a block reads: other nodes' samples, this node's own past before the block, and the
/// grid it steps on.
pub(super) struct Here<'a> {
    pub(super) reads: &'a [SampleView<'a>],
    pub(super) own: SampleView<'a>,
    pub(super) grid: Grid,
}

impl<'a> Here<'a> {
    fn source(&self, slot: Slot, n: i64, fresh: Fresh<'a>) -> Source<'a> {
        match slot {
            Slot::Read(id) => Source {
                view: self.reads[id.0 as usize],
                limit: None,
                fresh,
            },
            Slot::Own => Source {
                view: self.own,
                limit: Some(n),
                fresh,
            },
        }
    }
}

/// Samples `[from, from + len)`: every op the block's recurrence leaves out over the block
/// before the next, then the recurrent ops sample after sample. How many samples hold, and
/// the refusal a sample-at-a-time run meets first: the earliest sample, and within it the
/// earliest op.
pub(super) fn run(
    p: &CompiledOps,
    block: &mut BlockScratch,
    here: &Here,
    states: &mut [State],
    (from, len): (i64, usize),
) -> (usize, Option<SampleError>) {
    #[cfg(test)]
    super::counts::PASSES.with(|n| n.set(n.get() + 1));
    for (i, t) in block.times[..len].iter_mut().enumerate() {
        *t = here.grid.instant(from + i as i64);
    }
    let mut first: Option<(usize, usize, SampleError)> = None;
    let held = |first: &Option<(usize, usize, SampleError)>| first.as_ref().map_or(len, |f| f.0);
    let (recurrent, whole): (Vec<usize>, Vec<usize>) =
        (0..p.ops.len()).partition(|s| block.recurrent[*s]);
    for slot in whole {
        if let Err((at, e)) = fill(p, slot, block, here, states, (from, 0, held(&first))) {
            first = Some((at, slot, e));
        }
    }
    'samples: for i in 0..len {
        for &slot in &recurrent {
            if first
                .as_ref()
                .is_some_and(|(at, op, _)| (i, slot) > (*at, *op))
            {
                break 'samples;
            }
            if let Err((at, e)) = fill(p, slot, block, here, states, (from, i, i + 1)) {
                first = Some((at, slot, e));
                break 'samples;
            }
        }
    }
    (held(&first), first.map(|(_, _, e)| e))
}

type Refused = (usize, SampleError);

/// Sample `i` of the block, at index `n`, into its components.
type Sample<'a> = dyn FnMut(usize, i64, &mut [f64]) -> Result<(), SampleError> + 'a;

/// `f` at each sample in order from the block's sample `at`, and the first it refuses at.
fn each(
    out: &mut [f64],
    (from, at, w): (i64, usize, usize),
    f: &mut Sample,
) -> Result<(), Refused> {
    for (k, sample) in out.chunks_exact_mut(w).enumerate() {
        let i = at + k;
        f(i, from + i as i64, sample).map_err(|e| (i, e))?;
    }
    Ok(())
}

/// An operand's samples over the block, and its width.
type Operand<'a> = (&'a [f64], usize);

/// `out` folded with `a` component by component, a mono `a` spread across `out`'s width. Whole
/// slices where the widths agree, so the loop vectorises; each value sees the same operations
/// in the same order either way.
fn fold(out: &mut [f64], w: usize, (a, aw): Operand, f: impl Fn(f64, f64) -> f64) {
    match aw == w {
        true => out.iter_mut().zip(a).for_each(|(o, &x)| *o = f(*o, x)),
        false => {
            for (s, v) in out.chunks_exact_mut(w).zip(a.chunks_exact(aw)) {
                for (c, o) in s.iter_mut().enumerate() {
                    *o = f(*o, part(v, c));
                }
            }
        }
    }
}

/// `f` of `a` and `b` into `out`, as `fold` spreads and vectorises.
fn binary(
    out: &mut [f64],
    w: usize,
    (a, aw): Operand,
    (b, bw): Operand,
    f: impl Fn(f64, f64) -> f64,
) {
    match aw == w && bw == w {
        true => {
            for ((o, &x), &y) in out.iter_mut().zip(a).zip(b) {
                *o = f(x, y);
            }
        }
        false => {
            let pairs = a.chunks_exact(aw).zip(b.chunks_exact(bw));
            for (s, (u, v)) in out.chunks_exact_mut(w).zip(pairs) {
                for (c, o) in s.iter_mut().enumerate() {
                    *o = f(part(u, c), part(v, c));
                }
            }
        }
    }
}

/// Samples `[start, end)` of the block, of one slot.
fn fill(
    p: &CompiledOps,
    slot: usize,
    block: &mut BlockScratch,
    here: &Here,
    states: &mut [State],
    (from, start, end): (i64, usize, usize),
) -> Result<(), Refused> {
    #[cfg(test)]
    super::counts::FILLS.with(|n| n.set(n.get() + 1));
    let (op, w, len) = (&p.ops[slot], p.widths[slot], end - start);
    let (offsets, times, args) = (&block.offsets, &block.times[start..end], &p.args[slot]);
    let (done, rest) = block.values.split_at_mut(offsets[slot]);
    let (own, after) = rest.split_at_mut(offsets[slot + 1] - offsets[slot]);
    let (before, own) = own.split_at_mut(start * w);
    let out = &mut own[..w * len];
    let top = p.ops.len() - 1;
    let fresh = Fresh {
        from,
        values: match slot == top {
            true => before,
            false => &after[offsets[top] - offsets[slot + 1]..][..start * p.widths[top]],
        },
        width: p.widths[top],
    };
    let arg = |k: usize, i: usize| {
        let s = args[k];
        let w = p.widths[s];
        &done[offsets[s] + i * w..][..w]
    };
    let operand = |s: usize| {
        let w = p.widths[s];
        (&done[offsets[s] + start * w..][..w * len], w)
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
        Op::Wrap(wrap) => each(out, (from, start, w), &mut |_, n, s| {
            s[0] = wrap
                .and_then(|wrap| wrap.at(n))
                .ok_or(SampleError::UnreadablePosition)?;
            Ok(())
        }),
        Op::Noise { seed, at } => each(out, (from, start, w), &mut |_, n, s| {
            let step = at.ok_or(SampleError::UnreadablePosition)?.at(n);
            s[0] = sva_formula::draw(*seed, step);
            Ok(())
        }),
        Op::Indexed {
            slot, at, reach, ..
        } => each(out, (from, start, w), &mut |i, n, s| {
            let k = p.indices[*at]
                .at(n, here.grid, &|j| arg(*j, i)[0])
                .ok_or(SampleError::UnreadablePosition)?;
            if let Some((least, most)) = reach
                && !(*least..=*most).contains(&k.saturating_sub(n))
            {
                return Err(SampleError::ReadsAhead { at: k });
            }
            here.source(*slot, n, fresh).nearest(k, s)
        }),
        Op::Instant { at, .. } => each(out, (from, start, w), &mut |i, n, s| {
            let k = p.indices[*at]
                .at(n, here.grid, &|j| arg(*j, i)[0])
                .ok_or(SampleError::UnreadablePosition)?;
            s[0] = here.grid.instant(k);
            Ok(())
        }),
        Op::Read { slot, at } | Op::ReadScaled { slot, at, .. } => {
            let n = from + start as i64;
            let copied = here.source(*slot, n, fresh).copied(*at, n, (out, w));
            if !copied {
                each(out, (from, start, w), &mut |_, n, s| {
                    here.source(*slot, n, fresh).mapped(*at, n, s)
                })?;
            }
            if let Op::ReadScaled { by, .. } = op {
                out.iter_mut().for_each(|v| *v *= by);
            }
            Ok(())
        }
        Op::Formula { at } => each(out, (from, start, w), &mut |i, n, s| {
            for (c, v) in s.iter_mut().enumerate() {
                *v = p.formulas[*at]
                    .at(c, part(arg(0, i), c), here.grid)
                    .map_err(|_| SampleError::FormulaUnevaluable { at: n })?;
            }
            Ok(())
        }),
        Op::Add(_) => {
            out.fill(0.0);
            for &s in args {
                fold(out, w, operand(s), |acc, a| acc + a);
            }
            Ok(())
        }
        Op::Mul(_) => {
            out.fill(1.0);
            for &s in args {
                fold(out, w, operand(s), |acc, a| acc * a);
            }
            Ok(())
        }
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => {
            let (a, b) = (operand(args[0]), operand(args[1]));
            match op {
                Op::Sub => binary(out, w, a, b, |a, b| a - b),
                Op::Div => binary(out, w, a, b, |a, b| a / b),
                Op::Pow => binary(out, w, a, b, f64::powf),
                Op::Zip(f) => binary(out, w, a, b, |a, b| f.apply(a, b)),
                _ => unreachable!("the arm's own guard"),
            }
            Ok(())
        }
        Op::Map(f) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                for (c, v) in s.iter_mut().enumerate() {
                    *v = f.apply(part(arg(0, start + i), c));
                }
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
                let n = from + (start + i) as i64;
                let gain = match window.0 <= n && n < window.1 {
                    true => crate::collapse::shoulders(times[i], *a, *b, *rise, *fall),
                    false => 0.0,
                };
                for (c, v) in s.iter_mut().enumerate() {
                    *v = match gain {
                        0.0 => 0.0,
                        gain => part(arg(0, start + i), c) * gain,
                    };
                }
            }
            Ok(())
        }
        Op::Join(_) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                let mut c = 0;
                for k in 0..args.len() {
                    for &v in arg(k, start + i) {
                        s[c] = v;
                        c += 1;
                    }
                }
            }
            Ok(())
        }
        Op::Channel(k) => {
            for (i, v) in out.iter_mut().enumerate() {
                *v = arg(0, start + i)[*k];
            }
            Ok(())
        }
        Op::Filter { site, from: begins } => {
            let State::Filter(filter) = &mut states[site.0 as usize] else {
                unreachable!("a filter op names a filter site")
            };
            let sr = here.grid.sr();
            each(out, (from, start, w), &mut |i, n, s| {
                match n < *begins {
                    true => s.fill(0.0),
                    false => filter.process(arg(0, i), arg(1, i), arg(2, i), arg(3, i), s, sr, n),
                }
                Ok(())
            })
        }
        Op::Physics {
            site, from: begins, ..
        } => {
            let State::Physics(solver) = &mut states[site.0 as usize] else {
                unreachable!("a physics op names a physics site")
            };
            each(out, (from, start, w), &mut |i, n, s| {
                if n < *begins {
                    s.fill(0.0);
                    return Ok(());
                }
                let mut values = [0.0; crate::physics::MAX_VARYING];
                for (k, v) in values.iter_mut().enumerate().take(args.len()) {
                    *v = arg(k, i)[0] + 0.0;
                }
                s[0] = solver.step(&values[..args.len()])?;
                Ok(())
            })
        }
    }
}
