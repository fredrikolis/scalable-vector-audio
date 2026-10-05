// Concern: a written form's addends, the interval each is live in, and what walking it costs | Non-concern: evaluating one (column.rs) | IO: (&Body, grid) -> addends, intervals, (nodes, waves)

use sva_formula::closed_form::{Part, map_children};
use sva_formula::{Body, ClosedForm};

use super::active::{self, SampleInterval};
use super::{column, point};
use crate::grid::Grid;

pub(super) fn addends(form: &ClosedForm) -> Option<Vec<ClosedForm>> {
    if let Body::Add(parts) = &form.body {
        return Some(
            parts
                .iter()
                .map(|part| ClosedForm {
                    var: form.var,
                    body: (*part.body).clone(),
                    origin: part.origin,
                })
                .collect(),
        );
    }
    let under = linear_over(&form.body)?;
    let held = addends(&ClosedForm {
        body: under,
        ..form.clone()
    })?;
    Some(
        held.into_iter()
            .map(|addend| ClosedForm {
                body: map_children(&form.body, |_| {
                    Part::new(addend.origin, addend.body.clone())
                }),
                ..addend
            })
            .collect(),
    )
}

/// Each of these is linear, so over a sum it is the sum of itself over every addend.
fn linear_over(f: &Body) -> Option<Body> {
    match f {
        Body::Crop { of, .. }
        | Body::Shift { of, .. }
        | Body::Deriv { of, .. }
        | Body::Channel(of, _) => Some((*of.body).clone()),
        _ => None,
    }
}

/// One component's `(nodes, waves)` over the samples `[from, to)` of `grid`, priced as a
/// walk of the form: a run is priced by its Horner steps and turns its lines,
/// every other node is one, and a crop's operand counts only at the instants its interval holds.
pub(crate) fn point_work(
    f: &Body,
    component: usize,
    grid: Grid,
    span: SampleInterval,
) -> (u128, u128) {
    let clock = Clock::grid(grid);
    let Some(parts) = summed(f) else {
        return walked(f, component, &clock, span);
    };
    let n = (i128::from(span.1) - i128::from(span.0)).max(0) as u128;
    parts
        .iter()
        .zip(addend_intervals(&parts, grid))
        .fold((n, 0), |held, (part, live)| {
            let (priced, waves) = walked(&part.body, component, &clock, active::meet(span, live));
            (held.0 + priced, held.1 + waves)
        })
}

/// A written sum's addends in order, a left-nested `a + b + c` as one list: a sum
/// folds either from +0 to the same bits, as no partial sum from +0 is -0.
pub(super) fn summed(f: &Body) -> Option<Vec<&Part>> {
    let Body::Add(parts) = f else {
        return None;
    };
    let (mut head, mut tails) = (parts, Vec::new());
    while let [first, rest @ ..] = head.as_slice()
        && let Body::Add(inner) = &*first.body
    {
        tails.push(rest);
        head = inner;
    }
    let mut out: Vec<&Part> = head.iter().collect();
    for tail in tails.iter().rev() {
        out.extend(tail.iter());
    }
    Some(out)
}

pub(super) fn addend_intervals(parts: &[&Part], grid: Grid) -> Vec<SampleInterval> {
    parts
        .iter()
        .map(|part| live_interval(&part.body, grid))
        .collect()
}

pub(super) fn live_interval(f: &Body, grid: Grid) -> SampleInterval {
    live(f, &Clock::grid(grid), active::OPEN)
}

fn live(f: &Body, clock: &Clock, span: SampleInterval) -> SampleInterval {
    match f {
        Body::Crop { of, .. } => live(&of.body, clock, clock.cropped(span, f)),
        Body::Mul(parts) => parts
            .iter()
            .fold(span, |held, part| live(&part.body, clock, held)),
        Body::Shift { by, of } => live(&of.body, &clock.shifted(*by), span),
        _ => span,
    }
}

/// The instant a subterm is read at, from the grid's index: `None` once a warp moves it.
#[derive(Clone)]
struct Clock {
    grid: Grid,
    shifts: Option<Vec<f64>>,
}

impl Clock {
    fn grid(grid: Grid) -> Clock {
        Clock {
            grid,
            shifts: Some(Vec::new()),
        }
    }

    fn shifted(&self, by: f64) -> Clock {
        Clock {
            grid: self.grid,
            shifts: self
                .shifts
                .as_ref()
                .map(|held| [held.as_slice(), &[by]].concat()),
        }
    }

    /// The terms a banded series sums over the span, each instant's own count; where a warp
    /// moves the instants, the most any instant sums.
    fn summed(&self, b: &sva_formula::Banded, (from, to): SampleInterval) -> u128 {
        let Some(shifts) = &self.shifts else {
            return (to - from).max(0) as u128 * b.widest as u128;
        };
        let at = |n: i64| shifts.iter().fold(self.grid.instant(n), |t, by| t - by);
        let count = |n: i64| {
            let t = at(n);
            let turning = |rate| point::turning(rate, t).unwrap_or(f64::NAN);
            match turning(&b.slope).is_nan() || turning(&b.offset).is_nan() {
                true => b.widest as u128,
                false => b
                    .within(turning(&b.slope), turning(&b.offset))
                    .map_or(0, |(lo, hi)| (hi - lo + 1) as u128),
            }
        };
        (from..to).map(count).sum()
    }

    fn warped(&self) -> Clock {
        Clock {
            grid: self.grid,
            shifts: None,
        }
    }

    /// Where a crop's gain is not zero: inside `[l, r)` less the instants a shoulder opens at.
    fn cropped(&self, span: SampleInterval, crop: &Body) -> SampleInterval {
        let (
            Body::Crop {
                l, r, rise, fall, ..
            },
            Some(shifts),
        ) = (crop, &self.shifts)
        else {
            return span;
        };
        let at = |n: i64| shifts.iter().fold(self.grid.instant(n), |t, by| t - by);
        let (l, r) = (l.value(), r.value());
        let grid = shifts.is_empty().then_some(self.grid);
        let interval = match grid {
            Some(grid) => (grid.first_at(l), grid.first_at(r)),
            None => active::between(l, r, at),
        };
        let (mut from, mut to) = active::meet(span, interval);
        let shut = |n: i64| match grid {
            Some(_) => point::shoulders(at(n), l, r, *rise, *fall) == 0.0,
            None => point::crop_gain(at(n), l, r, *rise, *fall) == 0.0,
        };
        while from < to && shut(from) {
            from += 1;
        }
        while from < to && shut(to - 1) {
            to -= 1;
        }
        (from, to)
    }
}

fn walked(f: &Body, component: usize, clock: &Clock, span: SampleInterval) -> (u128, u128) {
    let n = (i128::from(span.1) - i128::from(span.0)).max(0) as u128;
    if n == 0 {
        return (0, 0);
    }
    let add = |a: (u128, u128), b: (u128, u128)| (a.0 + b.0, a.1 + b.1);
    let branch = |part: &Part, c: usize, clock: &Clock, span: SampleInterval| {
        add((n, 0), walked(&part.body, c, clock, span))
    };
    match f {
        Body::Run(run) => (super::run::steps(run) as u128 * n, run.len() as u128 * n),
        Body::Banded(b) => {
            let (priced, waves) = walked(&b.series.term.body, component, clock, span);
            let terms = clock.summed(b, span);
            (2 * n + priced * terms / n, waves * terms / n)
        }
        Body::Join(parts) => {
            let widths: Vec<usize> = parts
                .iter()
                .map(|p| column::width_of(&p.body, &[]))
                .collect();
            match point::lane_of(&widths, component) {
                Some((at, inner)) => branch(&parts[at], inner, clock, span),
                None => (n, 0),
            }
        }
        Body::Channel(of, k) => branch(of, usize::from(*k), clock, span),
        Body::Crop { of, .. } => branch(of, component, clock, clock.cropped(span, f)),
        // A product evaluates no factor past one a shut crop zeroed.
        Body::Mul(parts) => {
            let mut live = span;
            parts.iter().fold((n, 0), |held, part| {
                let walked = walked(&part.body, component, clock, live);
                live = clock.cropped(live, &part.body);
                add(held, walked)
            })
        }
        Body::Shift { by, of } => branch(of, component, &clock.shifted(*by), span),
        Body::Warp { at, of } => add(
            branch(at, component, clock, span),
            walked(&of.body, component, &clock.warped(), span),
        ),
        other => sva_formula::closed_form::children(other)
            .iter()
            .map(|part| walked(&part.body, component, clock, span))
            .fold((n, 0), add),
    }
}

/// One operation per written subterm, each read of `refs[k]` counting that form's.
pub(crate) fn terms(body: &Body, refs: &[usize]) -> usize {
    match body {
        Body::Node(id) => refs[id.0 as usize],
        Body::Banded(b) => (b.widest.max(0) as usize)
            .saturating_mul(terms(&b.series.term.body, refs))
            .saturating_add(2),
        _ => sva_formula::closed_form::children(body)
            .iter()
            .fold(1usize, |held, p| held.saturating_add(terms(&p.body, refs))),
    }
}
