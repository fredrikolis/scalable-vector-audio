// Concern: runs each op of one program over a block of samples, slot after slot | Non-concern: which program runs where, or how long a block may be | IO: (Program, reads, states) -> each slot's samples

use super::ops::Op;
use super::read::Source;
use super::renderer::{Grid, Slot};
use super::tape::Window;
use super::{Program, State, part};
use crate::error::SampleError;

pub(super) const BLOCK: usize = 128;

/// Every slot's samples over one block, component after component within each sample, and
/// each sample's instant.
pub(super) struct Block {
    values: Vec<f64>,
    offsets: Vec<usize>,
    times: Vec<f64>,
}

impl Block {
    pub(super) fn of(widths: &[usize]) -> Block {
        let mut offsets = Vec::with_capacity(widths.len() + 1);
        let mut end = 0;
        for w in widths {
            offsets.push(end);
            end += w * BLOCK;
        }
        offsets.push(end);
        Block {
            values: vec![0.0; end],
            offsets,
            times: vec![0.0; BLOCK],
        }
    }

    /// Sample `i` of the program's last slot.
    pub(super) fn top(&self, p: &Program, i: usize) -> &[f64] {
        let slot = p.ops.len() - 1;
        let w = p.widths[slot];
        &self.values[self.offsets[slot] + i * w..][..w]
    }
}

/// What a block reads: other nodes' samples, this node's own past before the block, and the
/// grid it steps on.
pub(super) struct Here<'a> {
    pub(super) reads: &'a [Window<'a>],
    pub(super) own: Window<'a>,
    pub(super) grid: Grid,
}

impl Here<'_> {
    fn source(&self, slot: Slot, n: i64) -> Source<'_> {
        match slot {
            Slot::Read(id) => Source {
                window: self.reads[id.0 as usize],
                limit: None,
            },
            Slot::Own => Source {
                window: self.own,
                limit: Some(n),
            },
        }
    }
}

/// Samples `[from, from + len)`, every op over the block before the next. How many samples
/// hold, and the refusal the first sample short of that met: each op runs only up to the
/// sample some op before it refused at, so that refusal is the one a sample-at-a-time run
/// meets first.
pub(super) fn run(
    p: &Program,
    block: &mut Block,
    here: &Here,
    states: &mut [State],
    (from, len): (i64, usize),
) -> (usize, Option<SampleError>) {
    for (i, t) in block.times[..len].iter_mut().enumerate() {
        *t = here.grid.instant(from + i as i64);
    }
    let (mut held, mut refused) = (len, None);
    for slot in 0..p.ops.len() {
        if let Err((at, e)) = fill(p, slot, block, here, states, (from, held)) {
            (held, refused) = (at, Some(e));
        }
    }
    (held, refused)
}

type Refused = (usize, SampleError);

/// Sample `i` of the block, at index `n`, into its components.
type Sample<'a> = dyn FnMut(usize, i64, &mut [f64]) -> Result<(), SampleError> + 'a;

/// `f` at each sample in order, and the first it refuses at.
fn each(out: &mut [f64], (from, w): (i64, usize), f: &mut Sample) -> Result<(), Refused> {
    for (i, sample) in out.chunks_exact_mut(w).enumerate() {
        f(i, from + i as i64, sample).map_err(|e| (i, e))?;
    }
    Ok(())
}

fn fill(
    p: &Program,
    slot: usize,
    block: &mut Block,
    here: &Here,
    states: &mut [State],
    (from, len): (i64, usize),
) -> Result<(), Refused> {
    let (op, w) = (&p.ops[slot], p.widths[slot]);
    let (done, rest) = block.values.split_at_mut(block.offsets[slot]);
    let out = &mut rest[..w * len];
    let (offsets, times, args) = (&block.offsets, &block.times, &p.args[slot]);
    let arg = |k: usize, i: usize| {
        let s = args[k];
        let w = p.widths[s];
        &done[offsets[s] + i * w..][..w]
    };
    match op {
        Op::Const(v) => {
            out.fill(*v);
            Ok(())
        }
        Op::Time => {
            out.copy_from_slice(&times[..len]);
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
            let k = p.indices[*at]
                .at(n, here.grid, &|j| arg(*j, i)[0])
                .ok_or(SampleError::UnreadablePosition)?;
            if let Some((least, most)) = reach
                && !(*least..=*most).contains(&k.saturating_sub(n))
            {
                return Err(SampleError::ReadsAhead { at: k });
            }
            here.source(*slot, n).nearest(k, s)
        }),
        Op::Instant { at, .. } => each(out, (from, w), &mut |i, n, s| {
            let k = p.indices[*at]
                .at(n, here.grid, &|j| arg(*j, i)[0])
                .ok_or(SampleError::UnreadablePosition)?;
            s[0] = here.grid.instant(k);
            Ok(())
        }),
        Op::Read { slot, at } => each(out, (from, w), &mut |_, n, s| {
            here.source(*slot, n).mapped(*at, n, s)
        }),
        Op::ReadScaled { slot, at, by } => each(out, (from, w), &mut |_, n, s| {
            here.source(*slot, n).mapped(*at, n, s)?;
            for v in s.iter_mut() {
                *v *= by;
            }
            Ok(())
        }),
        Op::Formula { at } => each(out, (from, w), &mut |i, n, s| {
            for (c, v) in s.iter_mut().enumerate() {
                *v = p.formulas[*at]
                    .at(c, part(arg(0, i), c))
                    .map_err(|_| SampleError::FormulaUnevaluable { at: n })?;
            }
            Ok(())
        }),
        Op::Add(_) | Op::Mul(_) => {
            let product = matches!(op, Op::Mul(_));
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                for (c, v) in s.iter_mut().enumerate() {
                    *v = (0..args.len()).fold(
                        f64::from(u8::from(product)),
                        |acc, k| match product {
                            true => acc * part(arg(k, i), c),
                            false => acc + part(arg(k, i), c),
                        },
                    );
                }
            }
            Ok(())
        }
        Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                for (c, v) in s.iter_mut().enumerate() {
                    let (a, b) = (part(arg(0, i), c), part(arg(1, i), c));
                    *v = match op {
                        Op::Sub => a - b,
                        Op::Div => a / b,
                        Op::Pow => a.powf(b),
                        Op::Zip(f) => f.apply(a, b),
                        _ => unreachable!("the arm's own guard"),
                    };
                }
            }
            Ok(())
        }
        Op::Map(f) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                for (c, v) in s.iter_mut().enumerate() {
                    *v = f.apply(part(arg(0, i), c));
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
                let n = from + i as i64;
                let gain = match window.0 <= n && n < window.1 {
                    true => crate::collapse::shoulders(times[i], *a, *b, *rise, *fall),
                    false => 0.0,
                };
                for (c, v) in s.iter_mut().enumerate() {
                    *v = match gain {
                        0.0 => 0.0,
                        gain => part(arg(0, i), c) * gain,
                    };
                }
            }
            Ok(())
        }
        Op::Join(_) => {
            for (i, s) in out.chunks_exact_mut(w).enumerate() {
                let mut c = 0;
                for k in 0..args.len() {
                    for &v in arg(k, i) {
                        s[c] = v;
                        c += 1;
                    }
                }
            }
            Ok(())
        }
        Op::Channel(k) => {
            for (i, v) in out.iter_mut().enumerate() {
                *v = arg(0, i)[*k];
            }
            Ok(())
        }
        Op::Filter { site, from: start } => {
            let State::Filter(filter) = &mut states[site.0 as usize] else {
                unreachable!("a filter op names a filter site")
            };
            let sr = here.grid.sr();
            each(out, (from, w), &mut |i, n, s| {
                match n < *start {
                    true => s.fill(0.0),
                    false => filter.process(arg(0, i), arg(1, i), arg(2, i), arg(3, i), s, sr, n),
                }
                Ok(())
            })
        }
        Op::Physics {
            site, from: start, ..
        } => {
            let State::Physics(solver) = &mut states[site.0 as usize] else {
                unreachable!("a physics op names a physics site")
            };
            each(out, (from, w), &mut |i, n, s| {
                if n < *start {
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
