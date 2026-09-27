// Concern: where on the grid each atom of a lane can be nonzero, and a lane summed over only those | Non-concern: one atom's value (point.rs), choosing the row | IO: (&Lane, span) -> windows, a plane

use sva_formula::spectral_sum::atom::{Gauss, SpectralAtom};
use sva_formula::{Lane, exp_zero_at};

use super::point;
use crate::error::CollapseError;

pub(crate) type Window = (i64, i64);

pub(crate) const OPEN: Window = (i64::MIN, i64::MAX);

const REACH: i64 = 1 << 62;

const FINITE: f64 = 1e300;

fn at(n: i64, step: f64) -> f64 {
    n as f64 * step
}

/// Outside it `smooth_at` is exactly zero: its indicator, or a factor the engine's own `exp`
/// underflows while every other factor stays finite.
pub(crate) fn window(a: &SpectralAtom, step: f64) -> Window {
    if a.is_delta() {
        return OPEN;
    }
    let mut held = OPEN;
    if let Some(ind) = a.ind {
        let (l, r) = (ind.l.value(), ind.r.value());
        held = meet(
            held,
            (start(|n| l <= at(n, step)), end(|n| r <= at(n, step))),
        );
    }
    if a.pole.is_some() || !reaches_finite(a, step) {
        return held;
    }
    let zero = exp_zero_at();
    let exp = a.exp.filter(|e| e.sigma != 0.0);
    if let Some(e) = exp {
        let dead = |n: i64| e.sigma * (at(n, step) - e.mu) <= zero;
        held = meet(
            held,
            match e.sigma < 0.0 {
                true => (i64::MIN, end(dead)),
                false => (start(|n| !dead(n)), i64::MAX),
            },
        );
    }
    if let Some(g) = a.gauss {
        let grows = |n: i64, rising: bool| {
            exp.is_some_and(|e| (e.sigma > 0.0) == rising || e.sigma * (at(n, step) - e.mu) > 700.0)
        };
        held = meet(held, gaussian(g, step, zero, &grows));
    }
    held
}

fn gaussian(g: Gauss, step: f64, zero: f64, grows: &dyn Fn(i64, bool) -> bool) -> Window {
    let dead = |n: i64| {
        let x = at(n, step);
        -g.a * (x - g.mu) * (x - g.mu) <= zero
    };
    let centre = start(|n| at(n, step) >= g.mu).clamp(-REACH, REACH);
    let right = first_in(centre, REACH, dead).filter(|n| !grows(*n, true));
    let left =
        first_in(-REACH, centre, |n| !dead(n)).filter(|n| *n > -REACH && !grows(n - 1, false));
    (left.unwrap_or(i64::MIN), right.unwrap_or(i64::MAX))
}

fn reaches_finite(a: &SpectralAtom, step: f64) -> bool {
    let farthest = (i64::MAX as f64) * step.abs();
    (a.c.re.abs() + a.c.im.abs()) * farthest.powi(i32::from(a.poly)) < FINITE
}

fn meet(a: Window, b: Window) -> Window {
    let held = (a.0.max(b.0), a.1.min(b.1));
    match held.0 < held.1 {
        true => held,
        false => (0, 0),
    }
}

fn first_in(lo: i64, hi: i64, holds: impl Fn(i64) -> bool) -> Option<i64> {
    if lo > hi || !holds(hi) {
        return None;
    }
    let (mut no, mut yes) = (lo, hi);
    if holds(lo) {
        return Some(lo);
    }
    while i128::from(yes) - i128::from(no) > 1 {
        let mid = ((i128::from(no) + i128::from(yes)) / 2) as i64;
        match holds(mid) {
            true => yes = mid,
            false => no = mid,
        }
    }
    Some(yes)
}

fn start(holds: impl Fn(i64) -> bool) -> i64 {
    match first_in(-REACH, REACH, holds) {
        Some(n) if n == -REACH => i64::MIN,
        Some(n) => n,
        None => REACH,
    }
}

fn end(holds: impl Fn(i64) -> bool) -> i64 {
    match first_in(-REACH, REACH, holds) {
        Some(n) if n == -REACH => i64::MIN,
        Some(n) => n,
        None => i64::MAX,
    }
}

pub(crate) fn windows(lane: &Lane, step: f64) -> Vec<Window> {
    lane.atoms.iter().map(|a| window(a, step)).collect()
}

/// Samples `[from, to)` of one lane into `out`, each summed in lane order over the atoms
/// live there: one left out adds an exact zero to a sum that starts at +0.
pub(crate) fn sweep(
    lane: &Lane,
    windows: &[Window],
    (from, to): Window,
    step: f64,
    out: &mut [f64],
) -> Result<(), CollapseError> {
    let mut order: Vec<usize> = (0..windows.len())
        .filter(|&i| windows[i].0 < windows[i].1 && windows[i].0 < to && windows[i].1 > from)
        .collect();
    order.sort_by_key(|&i| windows[i].0);
    let (mut live, mut next, mut n) = (Vec::<usize>::new(), 0, from);
    while n < to {
        while let Some(&i) = order.get(next).filter(|&&i| windows[i].0 <= n) {
            let slot = live.partition_point(|&j| j < i);
            live.insert(slot, i);
            next += 1;
        }
        live.retain(|&i| windows[i].1 > n);
        let stop = live
            .iter()
            .map(|&i| windows[i].1)
            .chain(order.get(next).map(|&i| windows[i].0))
            .fold(to, i64::min);
        for m in n..stop {
            out[(m - from) as usize] = point::eval_among(lane, &live, at(m, step))?.re;
        }
        n = stop;
    }
    Ok(())
}

pub(crate) fn evaluated(windows: &[Window], spans: &[Window]) -> u128 {
    windows
        .iter()
        .map(|w| {
            spans
                .iter()
                .map(|s| (w.1.min(s.1) as i128 - w.0.max(s.0) as i128).max(0) as u128)
                .sum::<u128>()
        })
        .sum()
}

pub(crate) fn hull(windows: &[Window], spans: &[Window]) -> Option<Window> {
    let mut held: Option<Window> = None;
    for w in windows {
        for s in spans {
            let met = meet(*w, *s);
            if met.0 < met.1 {
                held = Some(held.map_or(met, |h| (h.0.min(met.0), h.1.max(met.1))));
            }
        }
    }
    held
}
