// Concern: reads a closed form onto any span of the grid by rows no horizon chooses | Non-concern: the rows a whole render fits to its horizon (plan.rs) | IO: (form, rate) -> Rows; (Tape, to) -> samples

use sva_formula::{ClosedForm, SpectralSum, Var, normalize_closed_form};

use super::lines::Direct;
use super::truncate::{self, Audible};
use super::{plan, point, reaches_no_atom, span};
use crate::error::CollapseError;
use crate::machine::tape::Tape;
use crate::profile::Profile;

/// Rows chosen from the form, the rate and the profile alone: each sample is its own
/// instant's value, so any span reads what one whole span reads there.
pub struct Rows {
    row: Row,
    rate: u32,
    width: usize,
}

enum Row {
    /// Every kept line summed at each instant, as a whole render's direct route sums them.
    Lines(Vec<Option<Direct>>),
    Sweep {
        sum: Box<SpectralSum>,
        spans: Vec<Option<Vec<(usize, usize)>>>,
    },
    Point {
        written: Box<ClosedForm>,
        width: usize,
    },
    Added(Vec<(Row, usize)>),
}

impl Rows {
    /// `collapse::render`'s dispatch.
    pub fn of(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Rows, CollapseError> {
        match normalize_closed_form(form) {
            Ok(sum) => Rows::of_spectral_sum_or_point(&sum, Some(form), rate, profile),
            Err(_) if form.var == Var::T => Ok(Rows::held(of_written(form, rate, profile)?, rate)),
            Err(left) => Err(CollapseError::LeftAlgebra(left.reason.clause())),
        }
    }

    pub fn of_spectral_sum_or_point(
        sum: &SpectralSum,
        written: Option<&ClosedForm>,
        rate: u32,
        profile: &Profile,
    ) -> Result<Rows, CollapseError> {
        let row = match of_sum(sum, rate, profile) {
            Err(e) if reaches_no_atom(&e) => match written.filter(|t| t.var == Var::T) {
                Some(form) => of_written(form, rate, profile),
                None => Err(e),
            },
            other => other,
        }?;
        Ok(Rows::held(row, rate))
    }

    fn held(row: Row, rate: u32) -> Rows {
        Rows {
            width: width(&row),
            row,
            rate,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// Appends `[tape.end(), to)` of every component, counted from the grid's start.
    pub fn extend(&self, to: usize, tape: &mut Tape) -> Result<(), CollapseError> {
        let step = 1.0 / f64::from(self.rate);
        for i in tape.end()..to {
            for c in 0..self.width {
                tape.push(c, value(&self.row, c, i, self.rate, step)?);
            }
        }
        Ok(())
    }
}

/// `plan::of` with each row a horizon picks by cost replaced by the one summed per instant.
fn of_sum(sum: &SpectralSum, rate: u32, profile: &Profile) -> Result<Row, CollapseError> {
    if sum.var == Var::F {
        return Err(CollapseError::NoBlockRow);
    }
    if let Some(found) = plan::kept_lines(sum, profile, profile.ceiling(rate))? {
        return Ok(Row::Lines(
            found.kept.iter().map(|kept| Direct::of(kept)).collect(),
        ));
    }
    let truncated = truncate::spectral_sum(sum, Audible::of(profile, rate))?;
    let spans = truncated
        .lanes
        .iter()
        .map(|lane| span::windows(lane, 0.0, rate, f64::INFINITY))
        .collect();
    Ok(Row::Sweep {
        sum: Box::new(truncated),
        spans,
    })
}

/// `plan::of_written`'s split into addends.
fn of_written(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Row, CollapseError> {
    let Some(addends) = plan::addends(form) else {
        return point(form, rate, profile);
    };
    let mut parts = Vec::with_capacity(addends.len());
    for addend in &addends {
        match of_term(addend, rate, profile)? {
            Row::Added(inner) => parts.extend(inner),
            part => {
                let held = width(&part);
                parts.push((part, held));
            }
        }
    }
    match parts
        .iter()
        .all(|(part, _)| matches!(part, Row::Point { .. }))
    {
        true => point(form, rate, profile),
        false => Ok(Row::Added(parts)),
    }
}

fn of_term(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Row, CollapseError> {
    let Ok(sum) = normalize_closed_form(form) else {
        return of_written(form, rate, profile);
    };
    match of_sum(&sum, rate, profile) {
        Err(nested @ CollapseError::NestedSeries { .. }) => Err(nested),
        Err(_) => of_written(form, rate, profile),
        held => held,
    }
}

fn point(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Row, CollapseError> {
    let written = ClosedForm {
        body: truncate::written(&form.body, Audible::of(profile, rate))?,
        ..form.clone()
    };
    let width = point::width_of(&written.body, &point::NoRefs).max(1);
    Ok(Row::Point {
        written: Box::new(written),
        width,
    })
}

fn width(row: &Row) -> usize {
    match row {
        Row::Lines(lanes) => lanes.len(),
        Row::Sweep { sum, .. } => sum.lanes.len(),
        Row::Point { width, .. } => *width,
        Row::Added(parts) => parts.iter().map(|(_, w)| *w).max().unwrap_or(1),
    }
}

/// Each row's arithmetic, in its whole-render row's order, so the bits agree.
fn value(row: &Row, c: usize, i: usize, rate: u32, step: f64) -> Result<f64, CollapseError> {
    Ok(match row {
        Row::Lines(lanes) => {
            let mut held = 0.0;
            if let Some(direct) = &lanes[c] {
                held += direct.at(0.0 + i as f64 / f64::from(rate));
            }
            held
        }
        Row::Sweep { sum, spans } => {
            let inside = spans[c]
                .as_ref()
                .is_none_or(|spans| spans.iter().any(|(from, to)| (*from..*to).contains(&i)));
            match inside {
                true => point::eval_lane(&sum.lanes[c], 0.0 + i as f64 * step)?.re,
                false => 0.0,
            }
        }
        Row::Point { written, .. } => {
            point::eval_body(&written.body, c, 0.0 + i as f64 * step, &point::NoRefs)?.re
        }
        Row::Added(parts) => {
            let mut sum = 0.0;
            for (part, held) in parts {
                let lane = match held {
                    1 => 0,
                    _ => c,
                };
                if lane < *held {
                    sum += value(part, lane, i, rate, step)?;
                }
            }
            sum
        }
    })
}
