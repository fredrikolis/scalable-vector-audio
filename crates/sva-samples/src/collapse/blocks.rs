// Concern: reads a closed form onto any span of the grid by rows no extent chooses, priced | Non-concern: rows a whole render fits to its extent | IO: (form, rate) -> Rows; (Tape, to) -> samples, work

use sva_formula::{ClosedForm, SpectralSum, Var, normalize_closed_form};

use super::active::{self, Window};
use super::lines::{self, Direct};
use super::truncate::{self, Audible};
use super::{atoms, plan, point, reaches_no_atom, span, tail};
use crate::error::CollapseError;
use crate::label::{Detail, Label, Rule, Source};
use crate::machine::tape::Tape;
use crate::profile::Profile;

/// Rows chosen from the form, the rate and the profile alone: each sample is its own
/// instant's value, so any span reads what one whole span reads there.
pub struct Rows {
    row: Row,
    rate: u32,
    width: usize,
    label: (Source, Detail),
}

enum Row {
    /// Every kept line summed at each instant.
    Lines(Vec<Option<Direct>>),
    Sweep {
        sum: Box<SpectralSum>,
        spans: Vec<Option<Vec<(i64, i64)>>>,
        windows: Vec<Vec<Window>>,
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

    fn held((row, label): (Row, (Source, Detail)), rate: u32) -> Rows {
        Rows {
            width: width(&row),
            row,
            rate,
            label,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// How exact these rows are, stated once for every sample they write.
    pub fn label(&self, profile: &Profile) -> Label {
        let (source, detail) = self.label.clone();
        Label::new(source, profile.name, self.rate, detail)
    }

    /// The lines a row of lines sums, each lane's, as hertz; `None` for any other row.
    pub fn lines(&self) -> Option<Vec<f64>> {
        let Row::Lines(lanes) = &self.row else {
            return None;
        };
        Some(lanes.iter().flatten().flat_map(Direct::hz).collect())
    }

    /// `[from, to)` of every component, on the grid's own index.
    pub fn planes(&self, from: i64, to: i64) -> Result<Vec<Vec<f64>>, CollapseError> {
        (0..self.width)
            .map(|c| values(&self.row, c, (from, to), self.rate))
            .collect()
    }

    /// What writing `[from, to)` of every component takes, as `(priced flops, waves)`.
    pub fn work(&self, from: i64, to: i64) -> (u128, u128) {
        (0..self.width).fold((0, 0), |held, c| {
            let (priced, waves) = worked(&self.row, c, from, to, 1.0 / f64::from(self.rate));
            (held.0 + priced, held.1 + waves)
        })
    }

    /// Appends `[tape.end(), to)` of every component, on the grid's own index.
    pub fn extend(&self, to: i64, tape: &mut Tape) -> Result<(), CollapseError> {
        let from = tape.end();
        if to <= from {
            return Ok(());
        }
        let planes = (0..self.width)
            .map(|c| values(&self.row, c, (from, to), self.rate))
            .collect::<Result<Vec<_>, _>>()?;
        for i in 0..(to - from) as usize {
            for (c, plane) in planes.iter().enumerate() {
                tape.push(c, plane[i]);
            }
        }
        Ok(())
    }
}

/// `(priced flops, waves)` one component's row takes over `[from, to)`: a line, a node walked
/// or an atom inside its spans, a sample each, as a whole render prices them.
fn worked(row: &Row, c: usize, from: i64, to: i64, step: f64) -> (u128, u128) {
    let n = (to - from) as u128;
    let times = |(priced, waves): (usize, usize), n: u128| (priced as u128 * n, waves as u128 * n);
    match row {
        Row::Lines(lanes) => lanes[c]
            .as_ref()
            .map_or((0, 0), |d| times(d.lines_priced_and_turned(), n)),
        Row::Sweep { spans, windows, .. } => {
            let evaluated =
                active::evaluated(&windows[c], &inside(spans[c].as_deref(), (from, to)));
            (evaluated, evaluated)
        }
        Row::Point { written, .. } => plan::point_work(&written.body, c, step, (from, to)),
        Row::Added(parts) => {
            parts
                .iter()
                .fold((0, 0), |held, (part, width)| match lane(*width, c) {
                    Some(lane) => {
                        let (priced, waves) = worked(part, lane, from, to, step);
                        (held.0 + priced, held.1 + waves)
                    }
                    None => held,
                })
        }
    }
}

type Labelled = (Row, (Source, Detail));

/// `plan::of` with each row an extent picks by cost replaced by the one summed per instant.
fn of_sum(sum: &SpectralSum, rate: u32, profile: &Profile) -> Result<Labelled, CollapseError> {
    if sum.var == Var::F {
        return Err(CollapseError::NoBlockRow);
    }
    let ceiling = profile.ceiling(rate);
    if let Some(found) = plan::kept_lines(sum, profile, ceiling)? {
        let (dropped, dropped_more) = lines::dropped_list(found.dropped());
        let summed = lines::distinct(&found.kept);
        let detail = Detail::Lines {
            rule: Rule::LineSpectrumSummed,
            placed: 0,
            summed,
            dropped,
            dropped_more,
            terms: Some(summed),
            tail_db: found.tail(),
        };
        let row = Row::Lines(found.kept.iter().map(|kept| Direct::of(kept)).collect());
        return Ok((row, (Source::Exact, detail)));
    }
    let truncated = truncate::spectral_sum(sum, Audible::of(profile, rate))?;
    let label = match () {
        () if atoms::band_limited(&truncated, ceiling, profile) => (
            Source::Exact,
            Detail::Continuous {
                rule: Rule::BandLimited,
            },
        ),
        () if atoms::windowed(&truncated) => (
            Source::Measured,
            Detail::Cropped {
                rule: Rule::CroppedPair,
                tail_db: tail::tail_db(&truncated, ceiling),
            },
        ),
        () => (
            Source::Measured,
            Detail::Point {
                rule: Rule::PointSampled,
                alias_db: None,
            },
        ),
    };
    let spans = truncated
        .lanes
        .iter()
        .map(|lane| span::windows(lane, rate))
        .collect();
    let step = 1.0 / f64::from(rate);
    let windows = truncated
        .lanes
        .iter()
        .map(|lane| active::windows(lane, step))
        .collect();
    let row = Row::Sweep {
        sum: Box::new(truncated),
        spans,
        windows,
    };
    Ok((row, label))
}

/// `plan::of_written`'s split into addends.
fn of_written(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Labelled, CollapseError> {
    let Some(addends) = plan::addends(form) else {
        return point(form, rate, profile);
    };
    let mut parts = Vec::with_capacity(addends.len());
    let mut labels: Vec<(Source, Detail)> = Vec::new();
    for addend in &addends {
        match of_term(addend, rate, profile)? {
            (Row::Added(inner), (source, Detail::Added { parts: details })) => {
                parts.extend(inner);
                labels.extend(details.into_iter().map(|d| (source, d)));
            }
            (part, (source, detail)) => {
                let held = width(&part);
                parts.push((part, held));
                labels.push((source, detail));
            }
        }
    }
    if parts
        .iter()
        .all(|(part, _)| matches!(part, Row::Point { .. }))
    {
        return point(form, rate, profile);
    }
    let source = match labels.iter().all(|(s, _)| *s == Source::Exact) {
        true => Source::Exact,
        false => Source::Measured,
    };
    let detail = Detail::Added {
        parts: labels.into_iter().map(|(_, d)| d).collect(),
    };
    Ok((Row::Added(parts), (source, detail)))
}

fn of_term(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Labelled, CollapseError> {
    let Ok(sum) = normalize_closed_form(form) else {
        return of_written(form, rate, profile);
    };
    match of_sum(&sum, rate, profile) {
        Err(nested @ CollapseError::NestedSeries { .. }) => Err(nested),
        Err(_) => of_written(form, rate, profile),
        held => held,
    }
}

fn point(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Labelled, CollapseError> {
    let written = ClosedForm {
        body: truncate::written(&form.body, Audible::of(profile, rate))?,
        ..form.clone()
    };
    let width = point::width_of(&written.body, &point::NoRefs).max(1);
    let row = Row::Point {
        written: Box::new(written),
        width,
    };
    let detail = Detail::Point {
        rule: Rule::PointSampled,
        alias_db: None,
    };
    Ok((row, (Source::Measured, detail)))
}

fn width(row: &Row) -> usize {
    match row {
        Row::Lines(lanes) => lanes.len(),
        Row::Sweep { sum, .. } => sum.lanes.len(),
        Row::Point { width, .. } => *width,
        Row::Added(parts) => parts.iter().map(|(_, w)| *w).max().unwrap_or(1),
    }
}

/// Each row's arithmetic over `[from, to)`, in its whole-render row's order, so the bits agree.
fn values(row: &Row, c: usize, (from, to): Window, rate: u32) -> Result<Vec<f64>, CollapseError> {
    let step = 1.0 / f64::from(rate);
    let n = (to - from) as usize;
    Ok(match row {
        Row::Lines(lanes) => (from..to)
            .map(|i| {
                let mut held = 0.0;
                if let Some(direct) = &lanes[c] {
                    held += direct.at(i as f64 / f64::from(rate));
                }
                held
            })
            .collect(),
        Row::Sweep {
            sum,
            spans,
            windows,
        } => {
            let mut out = vec![0.0; n];
            for (a, b) in inside(spans[c].as_deref(), (from, to)) {
                let at = (a - from) as usize..(b - from) as usize;
                active::sweep(&sum.lanes[c], &windows[c], (a, b), step, &mut out[at])?;
            }
            out
        }
        Row::Point { written, .. } => (from..to)
            .map(|i| Ok(point::eval_body(&written.body, c, i as f64 * step, &point::NoRefs)?.re))
            .collect::<Result<_, CollapseError>>()?,
        Row::Added(parts) => {
            let mut sum = vec![0.0; n];
            for (part, held) in parts {
                if let Some(lane) = lane(*held, c) {
                    for (out, v) in sum.iter_mut().zip(values(part, lane, (from, to), rate)?) {
                        *out += v;
                    }
                }
            }
            sum
        }
    })
}

/// `[from, to)` met with a lane's spans, where every atom of it is windowed.
fn inside(spans: Option<&[(i64, i64)]>, (from, to): Window) -> Vec<Window> {
    match spans {
        None => vec![(from, to)],
        Some(spans) => spans
            .iter()
            .map(|(a, b)| (from.max(*a), to.min(*b)))
            .filter(|(a, b)| a < b)
            .collect(),
    }
}

/// One component broadcasts.
fn lane(width: usize, c: usize) -> Option<usize> {
    let lane = if width == 1 { 0 } else { c };
    (lane < width).then_some(lane)
}
