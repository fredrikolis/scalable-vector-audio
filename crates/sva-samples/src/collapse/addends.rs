// Concern: a written form's addends and the interval each is live in | Non-concern: evaluating one (column.rs) | IO: (&Body, grid) -> addends, intervals

use sva_formula::closed_form::{Part, map_children};
use sva_formula::{Body, ClosedForm};

use super::active::{self, SampleInterval};
use super::point;
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

/// The instant a subterm is read at, from the grid's index.
#[derive(Clone)]
struct Clock {
    grid: Grid,
    shifts: Vec<f64>,
}

impl Clock {
    fn grid(grid: Grid) -> Clock {
        Clock {
            grid,
            shifts: Vec::new(),
        }
    }

    fn shifted(&self, by: f64) -> Clock {
        Clock {
            grid: self.grid,
            shifts: [self.shifts.as_slice(), &[by]].concat(),
        }
    }

    /// Where a crop's gain is not zero: inside `[l, r)` less the instants a shoulder opens at.
    fn cropped(&self, span: SampleInterval, crop: &Body) -> SampleInterval {
        let Body::Crop {
            l, r, rise, fall, ..
        } = crop
        else {
            return span;
        };
        let shifts = &self.shifts;
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
