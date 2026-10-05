// Concern: a block's recurrent ops as a flat tape, compiled per recurrent set, run sample by sample | Non-concern: which ops recur, the whole-block ops | IO: (CompiledOps, recurrent) -> Tape, samples

use super::kernels;
use super::ops::Op;
use super::read::Here;
use super::renderer::Slot;
use super::{CompiledOps, State};
use crate::error::SampleError;

/// A slot's place in the block's values and its width.
#[derive(Clone, Copy, Default)]
struct Place {
    at: usize,
    w: usize,
}

impl Place {
    fn sample(self, values: &[f64], i: usize) -> &[f64] {
        &values[self.at + i * self.w..][..self.w]
    }
}

struct Step {
    slot: usize,
    op: Op,
    out: Place,
    args: Vec<Place>,
    /// An own read's whole shift, where its map is one.
    lag: Option<i64>,
}

/// The slots a block runs whole, and the recurrent rest as steps in slot order.
#[derive(Default)]
pub(super) struct Tape {
    recurrent: Vec<bool>,
    whole: Vec<usize>,
    steps: Vec<Step>,
    top: Place,
}

/// The slots before `at`, and sample `i` of the slot at `at`, `w` wide.
fn split(values: &mut [f64], at: usize, i: usize, w: usize) -> (&[f64], &mut [f64]) {
    let (done, rest) = values.split_at_mut(at);
    (done, &mut rest[i * w..][..w])
}

struct Cx<'a, 'b> {
    p: &'a CompiledOps,
    values: &'a mut [f64],
    times: &'a [f64],
    here: &'a Here<'b>,
    states: &'a mut [State],
    from: i64,
}

impl Tape {
    pub(super) fn of(p: &CompiledOps, offsets: &[usize], recurrent: &[bool]) -> Tape {
        let place = |s: usize| Place {
            at: offsets[s],
            w: p.widths[s],
        };
        let mut tape = Tape {
            recurrent: recurrent.to_vec(),
            top: place(p.ops.len() - 1),
            ..Tape::default()
        };
        for (slot, op) in p.ops.iter().enumerate() {
            if !recurrent[slot] {
                tape.whole.push(slot);
                continue;
            }
            let lag = match op {
                Op::Read { at, .. } | Op::ReadScaled { at, .. } => at.moved(),
                _ => None,
            };
            tape.steps.push(Step {
                slot,
                op: *op,
                out: place(slot),
                args: p.args[slot].iter().map(|s| place(*s)).collect(),
                lag,
            });
        }
        tape
    }

    pub(super) fn recurrent(&self) -> &[bool] {
        &self.recurrent
    }

    pub(super) fn whole(&self) -> &[usize] {
        &self.whole
    }

    /// Every step at each sample of `[from, from + len)` in turn, up to `stop`, the earliest
    /// sample and op a whole op refused at; the first step refusing, its sample and slot.
    pub(super) fn run(
        &self,
        p: &CompiledOps,
        (values, times): (&mut [f64], &[f64]),
        here: &Here,
        states: &mut [State],
        (from, len): (i64, usize),
        stop: Option<(usize, usize)>,
    ) -> Option<(usize, usize, SampleError)> {
        let mut cx = Cx {
            p,
            values,
            times,
            here,
            states,
            from,
        };
        for i in 0..stop.map_or(len, |(at, _)| at) {
            for step in &self.steps {
                if let Err(e) = self.step(step, i, &mut cx) {
                    return Some((i, step.slot, e));
                }
            }
        }
        let (at, op) = stop?;
        for step in self.steps.iter().take_while(|s| s.slot < op) {
            if let Err(e) = self.step(step, at, &mut cx) {
                return Some((at, step.slot, e));
            }
        }
        None
    }

    /// This node's own sample `k` into `out` at sample `i` of the block, `w` wide: written
    /// earlier in the block, or held before it.
    fn own(
        &self,
        cx: &mut Cx,
        i: usize,
        k: i64,
        (out, w): (usize, usize),
    ) -> Result<(), SampleError> {
        let n = cx.from + i as i64;
        match k.checked_sub(cx.from).map(usize::try_from) {
            Some(Ok(j)) if k < n => {
                let past = self.top.at + j * self.top.w;
                for c in 0..w {
                    cx.values[out + c] = cx.values[past + c.min(self.top.w - 1)];
                }
                Ok(())
            }
            _ => cx
                .here
                .source(Slot::Own, n)
                .nearest(k, &mut cx.values[out..out + w]),
        }
    }

    #[inline(always)]
    fn step(&self, step: &Step, i: usize, cx: &mut Cx) -> Result<(), SampleError> {
        let n = cx.from + i as i64;
        let Step {
            out: Place { at, w },
            ref args,
            ..
        } = *step;
        let o = at + i * w;
        let operand = |done, a: &Place| (a.sample(done, i), a.w);
        let index = |cx: &Cx, at: usize, reach| {
            let time = |j: &usize| args[*j].sample(cx.values, i)[0];
            kernels::indexed(&cx.p.indices[at], reach, (n, cx.here.grid), &time)
        };
        match step.op {
            Op::Read {
                slot: Slot::Own,
                at: map,
            }
            | Op::ReadScaled {
                slot: Slot::Own,
                at: map,
                ..
            } => {
                let k = match step.lag.and_then(|lag| n.checked_add(lag)) {
                    Some(k) => k,
                    None => map.at(n),
                };
                self.own(cx, i, k, (o, w))?;
                if let Op::ReadScaled { by, .. } = step.op {
                    cx.values[o..o + w].iter_mut().for_each(|v| *v *= by);
                }
            }
            Op::Indexed {
                slot, at, reach, ..
            } => {
                let k = index(cx, at, reach)?;
                match slot {
                    Slot::Own => self.own(cx, i, k, (o, w))?,
                    Slot::Read(_) => {
                        let read = cx.here.source(slot, n);
                        read.nearest(k, &mut cx.values[o..o + w])?;
                    }
                }
            }
            Op::Instant { at, .. } => cx.values[o] = cx.here.grid.instant(index(cx, at, None)?),
            Op::Formula { at: formula } => {
                let (grid, from) = (cx.here.grid, cx.from);
                let (done, out) = split(cx.values, at, i, w);
                let times = operand(done, &args[0]);
                kernels::formula(cx.p, formula, (out, w), times, grid, (from, i))
                    .map_err(|(_, e)| e)?;
            }
            Op::Add(_) => {
                let (done, out) = split(cx.values, at, i, w);
                kernels::sum(out, w, args.iter().map(|a| operand(done, a)));
            }
            Op::Mul(_) => {
                let (done, out) = split(cx.values, at, i, w);
                kernels::product(out, w, args.iter().map(|a| operand(done, a)));
            }
            Op::Sub | Op::Div | Op::Pow | Op::Zip(_) => {
                let (done, out) = split(cx.values, at, i, w);
                let (a, b) = (operand(done, &args[0]), operand(done, &args[1]));
                kernels::pair(step.op, out, w, a, b);
            }
            Op::Map(f) => {
                let (done, out) = split(cx.values, at, i, w);
                kernels::map(f, args[0].sample(done, i), out);
            }
            Op::Crop {
                window,
                a,
                b,
                rise,
                fall,
            } => {
                let t = cx.times[i];
                let (done, out) = split(cx.values, at, i, w);
                kernels::crop(
                    window,
                    [a, b, rise, fall],
                    (n, t),
                    args[0].sample(done, i),
                    out,
                );
            }
            Op::Join(_) => {
                let (done, out) = split(cx.values, at, i, w);
                kernels::join(args.iter().map(|a| a.sample(done, i)), out);
            }
            Op::Channel(k) => cx.values[o] = args[0].sample(cx.values, i)[k],
            Op::Filter { site, from } => {
                let State::Filter(filter) = &mut cx.states[site.0 as usize] else {
                    unreachable!("a filter op names a filter site")
                };
                let sr = cx.here.grid.sr();
                let (done, out) = split(cx.values, at, i, w);
                let operands = [0, 1, 2, 3].map(|k| args[k].sample(done, i));
                kernels::filter(filter, (from, n, sr), operands, out);
            }
            Op::Physics { site, from, .. } => {
                let State::Physics(solver) = &mut cx.states[site.0 as usize] else {
                    unreachable!("a physics op names a physics site")
                };
                let (done, out) = split(cx.values, at, i, w);
                let operands = args.iter().map(|a| a.sample(done, i));
                kernels::physics(solver.as_mut(), (from, n), operands, out)?;
            }
            Op::Const(_)
            | Op::Time
            | Op::Wrap(_)
            | Op::Noise { .. }
            | Op::Read { .. }
            | Op::ReadScaled { .. } => {
                unreachable!("an op reading no operand and not its own past never recurs")
            }
        }
        Ok(())
    }
}
