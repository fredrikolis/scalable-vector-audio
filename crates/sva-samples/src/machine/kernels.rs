// Concern: each op's arithmetic over its operands' samples, at one sample or across a block | Non-concern: where the samples live, which ops run when | IO: (op, operands' samples) -> its samples

use super::renderer::{Formula, Index, Unary};
use super::{CompiledOps, part};
use crate::error::SampleError;
use crate::filters::FilterSite;
use crate::grid::{Grid, Round};
use crate::machine::ops::Op;
use crate::physics::Solver;

pub(super) type Refused = (usize, SampleError);

pub(super) type Operand<'a> = (&'a [f64], usize);

/// A mono `a` spreads across `out`'s width; each value sees the same operations in the same
/// order either way.
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

pub(super) fn sum<'a>(out: &mut [f64], w: usize, operands: impl Iterator<Item = Operand<'a>>) {
    accumulate(out, w, operands, 0.0, |acc, x| acc + x);
}

pub(super) fn product<'a>(out: &mut [f64], w: usize, operands: impl Iterator<Item = Operand<'a>>) {
    accumulate(out, w, operands, 1.0, |acc, x| acc * x);
}

/// One mono sample folds in a register.
#[inline(always)]
fn accumulate<'a>(
    out: &mut [f64],
    w: usize,
    operands: impl Iterator<Item = Operand<'a>>,
    from: f64,
    f: impl Fn(f64, f64) -> f64,
) {
    match out {
        [one] => *one = operands.fold(from, |acc, (a, _)| f(acc, a[0])),
        _ => {
            out.fill(from);
            operands.for_each(|a| fold(out, w, a, &f));
        }
    }
}

pub(super) fn pair(op: Op, out: &mut [f64], w: usize, a: Operand, b: Operand) {
    match op {
        Op::Sub => binary(out, w, a, b, |a, b| a - b),
        Op::Div => binary(out, w, a, b, |a, b| a / b),
        Op::Pow => binary(out, w, a, b, f64::powf),
        Op::Zip(f) => binary(out, w, a, b, |a, b| f.apply(a, b)),
        other => unreachable!("{other:?} is no two-operand op"),
    }
}

pub(super) fn map(f: Unary, x: &[f64], out: &mut [f64]) {
    for (c, v) in out.iter_mut().enumerate() {
        *v = f.apply(part(x, c));
    }
}

pub(super) fn crop(
    window: (i64, i64),
    [a, b, rise, fall]: [f64; 4],
    (n, t): (i64, f64),
    x: &[f64],
    out: &mut [f64],
) {
    let gain = match window.0 <= n && n < window.1 {
        true => crate::collapse::shoulders(t, a, b, rise, fall),
        false => 0.0,
    };
    for (c, v) in out.iter_mut().enumerate() {
        *v = match gain {
            0.0 => 0.0,
            gain => part(x, c) * gain,
        };
    }
}

pub(super) fn join<'a>(parts: impl Iterator<Item = &'a [f64]>, out: &mut [f64]) {
    let mut c = 0;
    for part in parts {
        for &v in part {
            out[c] = v;
            c += 1;
        }
    }
}

pub(super) fn filter(
    site: &mut FilterSite,
    (begins, n, sr): (i64, i64, f64),
    [x, cutoff, q, gain]: [&[f64]; 4],
    out: &mut [f64],
) {
    match n < begins {
        true => out.fill(0.0),
        false => site.process(x, cutoff, q, gain, out, sr, n),
    }
}

pub(super) fn physics<'a>(
    solver: &mut dyn Solver,
    (begins, n): (i64, i64),
    args: impl Iterator<Item = &'a [f64]>,
    out: &mut [f64],
) -> Result<(), SampleError> {
    if n < begins {
        out.fill(0.0);
        return Ok(());
    }
    let mut values = [0.0; crate::physics::MAX_VARYING];
    let mut arity = 0;
    for (v, x) in values.iter_mut().zip(args) {
        *v = x[0] + 0.0;
        arity += 1;
    }
    out[0] = solver.step(&values[..arity])?;
    Ok(())
}

pub(super) fn indexed(
    index: &Index<usize>,
    reach: Option<(i64, i64)>,
    (n, grid): (i64, Grid),
    time: &impl Fn(&usize) -> f64,
) -> Result<i64, SampleError> {
    let k = index
        .at(n, grid, time)
        .ok_or(SampleError::UnreadablePosition)?;
    match reach {
        Some((least, most)) if !(least..=most).contains(&k.saturating_sub(n)) => {
            Err(SampleError::ReadsAhead { at: k })
        }
        _ => Ok(k),
    }
}

/// Each component at the instant its time operand names, from the block's sample `start`; the
/// earliest sample any component refuses at refuses.
pub(super) fn formula(
    p: &CompiledOps,
    at: usize,
    (out, w): (&mut [f64], usize),
    (times, tw): Operand,
    grid: Grid,
    (from, start): (i64, usize),
) -> Result<(), Refused> {
    let len = out.len() / w;
    let time = |k: usize, c: usize| part(&times[k * tw..][..tw], c);
    let mut first: Option<usize> = None;
    let mut refuse = |k: usize| first = Some(first.map_or(k, |f| f.min(k)));
    match &p.formulas[at] {
        (Formula::Written(_), programs) => {
            for (c, program) in programs.iter().enumerate() {
                let mut columns = program.columns(len);
                columns.grid = grid;
                for k in 0..len {
                    let t = time(k, c);
                    columns.t[k] = t;
                    columns.on[k] = landed(grid, t, from + (start + k) as i64);
                }
                let values = columns.root(0, (0, len));
                for k in 0..len {
                    match values.get(k) {
                        Ok(v) => out[k * w + c] = v.re,
                        Err(_) => refuse(k),
                    }
                }
            }
        }
        (Formula::Rows(rows), _) => {
            for k in 0..len {
                for c in 0..w {
                    match rows.at(c, time(k, c)) {
                        Ok(v) => out[k * w + c] = v,
                        Err(_) => refuse(k),
                    }
                }
            }
        }
        (Formula::Drawn { seed, rate }, _) => {
            for k in 0..len {
                for c in 0..w {
                    match Grid::of(*rate).step_at(time(k, c), Round::Even) {
                        Some(step) => out[k * w + c] = sva_formula::draw(*seed, step),
                        None => refuse(k),
                    }
                }
            }
        }
    }
    match first {
        Some(k) => Err((
            start + k,
            SampleError::FormulaUnevaluable {
                at: from + (start + k) as i64,
            },
        )),
        None => Ok(()),
    }
}

/// The sample `t` is the instant of, where it is one. Below 2^50 samples on a grid of whole
/// steps a sample's instant rounds back to that sample, so the guess `n` is checked directly.
fn landed(grid: Grid, t: f64, n: i64) -> Option<i64> {
    if grid.is_rate() && n.unsigned_abs() < 1 << 50 && grid.instant(n) == t {
        return Some(n);
    }
    grid.step_at(t, Round::Even)
        .filter(|m| grid.instant(*m) == t)
}
