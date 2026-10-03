// Concern: reads a closed form onto any span of the grid by rows no extent chooses, priced | Non-concern: rows a whole render fits to its extent | IO: (form, rate) -> Rows; (Tape, to) -> samples, work

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::{ClosedForm, Lane, Opaque, Reads, SpectralSum, Var, normalize_closed_form};

use super::active::{self, Window};
use super::lines::{self, Direct};
use super::truncate::{self, Audible};
use super::{atoms, plan, point, reaches_no_atom, reading, span, tail};
use crate::Grid;
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
    /// Each lane's lines summed as runs under each common factor, read once an instant.
    Grouped(Vec<Vec<Group>>),
    Sweep {
        sum: Box<SpectralSum>,
        spans: Vec<Option<Vec<(i64, i64)>>>,
        windows: Vec<Vec<Window>>,
    },
    /// A written sum's addends each read only inside its own window.
    Point {
        written: Box<ClosedForm>,
        width: usize,
        windows: Vec<Window>,
    },
    /// Each addend's row, its width, and the window outside which it writes +0.
    Added(Vec<(Row, usize, Window)>),
}

impl Rows {
    /// `collapse::render`'s dispatch.
    pub fn of(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Rows, CollapseError> {
        match normalize_closed_form(form) {
            Ok(sum) => Rows::of_spectral_sum_or_point(&sum, Some(form), (rate, profile), &Opaque),
            Err(_) if form.var == Var::T => Ok(Rows::held(of_written(form, rate, profile)?, rate)),
            Err(left) => Err(CollapseError::LeftAlgebra(left.reason.clause())),
        }
    }

    /// Each ref a series term holds read as the form `reads` names; `written` reads none.
    pub fn of_spectral_sum_or_point(
        sum: &SpectralSum,
        written: Option<&ClosedForm>,
        (rate, profile): (u32, &Profile),
        reads: &dyn Reads,
    ) -> Result<Rows, CollapseError> {
        let row = match of_sum(sum, rate, profile, reads) {
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
            let (priced, waves) = worked(&self.row, c, from, to, Grid::of(self.rate));
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
fn worked(row: &Row, c: usize, from: i64, to: i64, grid: Grid) -> (u128, u128) {
    let n = (to - from) as u128;
    let times = |(priced, waves): (usize, usize), n: u128| (priced as u128 * n, waves as u128 * n);
    match row {
        Row::Lines(lanes) => lanes[c]
            .as_ref()
            .map_or((0, 0), |d| times(d.lines_priced_and_turned(), n)),
        Row::Grouped(lanes) => lanes[c].iter().fold((0, 0), |held, g| {
            let (from, to) = active::meet((from, to), g.live);
            let (priced, waves) = g
                .direct
                .as_ref()
                .map_or((0, 0), Direct::lines_priced_and_turned);
            let (priced, waves) = times((priced + 1, waves), (to - from).max(0) as u128);
            (held.0 + priced, held.1 + waves)
        }),
        Row::Sweep { spans, windows, .. } => {
            let evaluated =
                active::evaluated(&windows[c], &inside(spans[c].as_deref(), (from, to)));
            (evaluated, evaluated)
        }
        Row::Point { written, .. } => plan::point_work(&written.body, c, grid, (from, to)),
        Row::Added(parts) => parts.iter().fold((0, 0), |held, (part, width, reach)| {
            match (lane(*width, c), active::meet((from, to), *reach)) {
                (Some(lane), (a, b)) if a < b => {
                    let (priced, waves) = worked(part, lane, a, b, grid);
                    (held.0 + priced, held.1 + waves)
                }
                _ => held,
            }
        }),
    }
}

type Labelled = (Row, (Source, Detail));

/// One factor, the window outside which it zeroes a finite sum, and its lines.
struct Group {
    factor: SpectralAtom,
    live: Window,
    direct: Option<Direct>,
}

/// `plan::LanePlan::Grouped` with every line summed: `None` where a lane is no line sum
/// under common factors.
fn grouped(
    sum: &SpectralSum,
    (band, grid): (Audible, Grid),
    reads: &dyn Reads,
) -> Option<Vec<Vec<Group>>> {
    let lane = |lane: &Lane| {
        let groups = lines::line_groups(lane, band, reads)?.into_iter();
        Some(
            groups
                .map(|(factor, held)| Group {
                    live: plan::group_window(&factor, &held, 1, grid),
                    direct: Direct::of(&held),
                    factor,
                })
                .collect(),
        )
    };
    sum.lanes.iter().map(lane).collect()
}

/// `plan::of` with each row an extent picks by cost replaced by the one summed per instant.
fn of_sum(
    sum: &SpectralSum,
    rate: u32,
    profile: &Profile,
    reads: &dyn Reads,
) -> Result<Labelled, CollapseError> {
    if sum.var == Var::F {
        return Err(CollapseError::NoBlockRow);
    }
    let ceiling = profile.ceiling(rate);
    if let Some(found) = plan::kept_lines(sum, profile, ceiling, reads)? {
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
    let truncated = truncate::spectral_sum_read(sum, Audible::of(profile, rate), reads)?;
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
                tail_db: None,
            },
        ),
    };
    let grid = Grid::of(rate);
    if let Some(lanes) = grouped(sum, (Audible::of(profile, rate), grid), reads) {
        return Ok((Row::Grouped(lanes), label));
    }
    let spans = truncated
        .lanes
        .iter()
        .map(|lane| span::windows(lane, rate))
        .collect();
    let windows = truncated
        .lanes
        .iter()
        .map(|lane| active::windows(lane, grid))
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
                let (held, reach) = (width(&part), reach(&part, rate));
                parts.push((part, held, reach));
                labels.push((source, detail));
            }
        }
    }
    if parts
        .iter()
        .all(|(part, ..)| matches!(part, Row::Point { .. }))
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
    match of_sum(&sum, rate, profile, &Opaque) {
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
    let tail_db = truncate::dropped_db(&written.body);
    let width = point::width_of(&written.body, &point::NoRefs).max(1);
    let windows = plan::summed(&written.body).map_or_else(Vec::new, |parts| {
        plan::addend_windows(&parts, Grid::of(rate))
    });
    let row = Row::Point {
        written: Box::new(written),
        width,
        windows,
    };
    let detail = Detail::Point {
        rule: Rule::PointSampled,
        alias_db: None,
        tail_db,
    };
    Ok((row, (Source::Measured, detail)))
}

fn width(row: &Row) -> usize {
    match row {
        Row::Lines(lanes) => lanes.len(),
        Row::Grouped(lanes) => lanes.len(),
        Row::Sweep { sum, .. } => sum.lanes.len(),
        Row::Point { width, .. } => *width,
        Row::Added(parts) => parts.iter().map(|(_, w, _)| *w).max().unwrap_or(1),
    }
}

/// Outside it a row writes +0 at every sample: a line never ends, a sweep ends with its spans
/// where every atom is windowed, a written form with its addends' crops.
fn reach(row: &Row, rate: u32) -> Window {
    let hull = |spans: &mut dyn Iterator<Item = Window>| {
        spans
            .filter(|(a, b)| a < b)
            .reduce(|x, y| (x.0.min(y.0), x.1.max(y.1)))
            .unwrap_or((0, 0))
    };
    match row {
        Row::Lines(_) => active::OPEN,
        Row::Grouped(lanes) => hull(&mut lanes.iter().flatten().map(|g| g.live)),
        Row::Sweep { spans, .. } if spans.iter().all(Option::is_some) => {
            hull(&mut spans.iter().flatten().flatten().copied())
        }
        Row::Sweep { .. } => active::OPEN,
        Row::Point {
            written, windows, ..
        } => match plan::summed(&written.body) {
            Some(_) => hull(&mut windows.iter().copied()),
            None => plan::live_window(&written.body, Grid::of(rate)),
        },
        Row::Added(parts) => hull(&mut parts.iter().map(|(.., reach)| *reach)),
    }
}

/// Each row's arithmetic over `[from, to)`, in its whole-render row's order, so the bits agree.
fn values(row: &Row, c: usize, (from, to): Window, rate: u32) -> Result<Vec<f64>, CollapseError> {
    let grid = Grid::of(rate);
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
        // `reading::under_a_window`'s order: each group's sum from +0, times its factor.
        Row::Grouped(lanes) => {
            let mut out = vec![0.0; n];
            for g in &lanes[c] {
                let (a, b) = active::meet((from, to), g.live);
                for m in a..b {
                    let summed = g
                        .direct
                        .as_ref()
                        .map_or(0.0, |d| 0.0 + d.at(m as f64 / f64::from(rate)));
                    out[(m - from) as usize] +=
                        summed * point::eval_atom(&g.factor, grid.instant(m))?.re;
                }
            }
            out
        }
        Row::Sweep {
            sum,
            spans,
            windows,
        } => {
            let mut out = vec![0.0; n];
            for (a, b) in inside(spans[c].as_deref(), (from, to)) {
                let at = (a - from) as usize..(b - from) as usize;
                active::sweep(&sum.lanes[c], &windows[c], (a, b), grid, &mut out[at])?;
            }
            out
        }
        Row::Point {
            written, windows, ..
        } => reading::written(&written.body, windows, c, (from, to), grid)?,
        Row::Added(parts) => {
            let mut sum = vec![0.0; n];
            for (part, held, reach) in parts {
                if let (Some(lane), (a, b)) = (lane(*held, c), active::meet((from, to), *reach))
                    && a < b
                {
                    let at = (a - from) as usize;
                    for (out, v) in sum[at..].iter_mut().zip(values(part, lane, (a, b), rate)?) {
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
