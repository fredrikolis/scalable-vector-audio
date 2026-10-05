// Concern: which row a closed form takes, read on any span of its grid or at any instant | Non-concern: a form's arithmetic | IO: (form, grid) -> Rows; (span, t) -> samples

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::{ClosedForm, Lane, Opaque, Reads, SpectralSum, Var, normalize_closed_form};

use super::active::{self, SampleInterval};
use super::column::{self, Program};
use super::lines::{self, Direct};
use super::truncate::{self, Audible};
use super::{addends, atoms, point, tail};
use crate::error::CollapseError;
use crate::grid::{Grid, Round};
use crate::label::{Detail, Label, Rule, Source};
use crate::profile::Profile;

/// Rows chosen from the form, the grid and the profile alone: each sample is its own
/// instant's value, so any span reads what one whole span reads there.
pub struct Rows {
    row: Row,
    grid: Grid,
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
        intervals: Vec<Vec<SampleInterval>>,
    },
    /// A written sum's addends each read only inside its own interval. Each component's
    /// program holds the whole form first, then each addend where it is a sum.
    Point {
        written: Box<ClosedForm>,
        width: usize,
        intervals: Vec<SampleInterval>,
        programs: Vec<Program>,
    },
    /// Each addend's row, its width, and the interval outside which it writes +0.
    Added(Vec<(Row, usize, SampleInterval)>),
}

impl std::fmt::Debug for Rows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rows")
            .field("grid", &self.grid)
            .field("width", &self.width)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Rows {
    pub fn of(form: &ClosedForm, grid: Grid, profile: &Profile) -> Result<Rows, CollapseError> {
        match normalize_closed_form(form) {
            Ok(sum) => Rows::of_spectral_sum_or_point(&sum, Some(form), (grid, profile), &Opaque),
            Err(_) if form.var == Var::T => Ok(Rows::held(of_written(form, grid, profile)?, grid)),
            Err(left) => Err(CollapseError::LeftAlgebra(left.reason.clause())),
        }
    }

    /// Each ref a series term holds read as the form `reads` names; `written` reads none.
    pub fn of_spectral_sum_or_point(
        sum: &SpectralSum,
        written: Option<&ClosedForm>,
        (grid, profile): (Grid, &Profile),
        reads: &dyn Reads,
    ) -> Result<Rows, CollapseError> {
        let row = match of_sum(sum, grid, profile, reads) {
            Err(e) if reaches_no_atom(&e) => match written.filter(|t| t.var == Var::T) {
                Some(form) => of_written(form, grid, profile),
                None => Err(e),
            },
            other => other,
        }?;
        Ok(Rows::held(row, grid))
    }

    fn held((row, label): (Row, (Source, Detail)), grid: Grid) -> Rows {
        Rows {
            width: width(&row),
            row,
            grid,
            label,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// How exact these rows are, stated once for every sample they write.
    pub fn label(&self, profile: &Profile) -> Label {
        let (source, detail) = self.label.clone();
        Label::new(source, profile.name, self.grid.rate, detail)
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
            .map(|c| values(&self.row, c, (from, to), self.grid))
            .collect()
    }

    /// One component at `t`, exactly a sample's value where `t` is that sample's instant.
    pub fn at(&self, c: usize, t: f64) -> Result<f64, CollapseError> {
        let c =
            lane(self.width, c).ok_or(CollapseError::NotEvaluable("a component past the width"))?;
        let landed = self.grid.step_at(t, Round::Even);
        match landed.filter(|n| self.grid.instant(*n) == t) {
            Some(n) => Ok(values(&self.row, c, (n, n + 1), self.grid)?[0]),
            None => between(&self.row, c, t),
        }
    }
}

type Labelled = (Row, (Source, Detail));

/// One factor, the interval outside which it zeroes a finite sum, and its lines.
struct Group {
    factor: SpectralAtom,
    live: SampleInterval,
    direct: Option<Direct>,
}

/// Each lane's lines under their common factors, summed: `None` where a lane is no line sum
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
                    live: lines::group_interval(&factor, &held, grid),
                    direct: Direct::of(&held),
                    factor,
                })
                .collect(),
        )
    };
    sum.lanes.iter().map(lane).collect()
}

/// The rows in FORMAT 9.1's order; the first guard that holds decides.
fn of_sum(
    sum: &SpectralSum,
    grid: Grid,
    profile: &Profile,
    reads: &dyn Reads,
) -> Result<Labelled, CollapseError> {
    if sum.var == Var::F {
        return Err(CollapseError::NoBlockRow);
    }
    let band = Audible::on(profile, grid);
    let ceiling = band.ceiling();
    if let Some(found) = lines::kept_lines(sum, profile, ceiling, reads)? {
        let (dropped, dropped_more) = lines::dropped_list(found.dropped());
        let summed = lines::distinct(&found.kept);
        let detail = Detail::Lines {
            rule: Rule::LineSpectrumSummed,
            summed,
            dropped,
            dropped_more,
            tail_db: found.tail(),
        };
        let row = Row::Lines(found.kept.iter().map(|kept| Direct::of(kept)).collect());
        return Ok((row, (Source::Exact, detail)));
    }
    let truncated = truncate::spectral_sum_read(sum, band, reads)?;
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
    if let Some(lanes) = grouped(sum, (band, grid), reads) {
        return Ok((Row::Grouped(lanes), label));
    }
    let spans = truncated
        .lanes
        .iter()
        .map(|lane| active::spans(lane, grid))
        .collect();
    let intervals = truncated
        .lanes
        .iter()
        .map(|lane| active::nonzero_intervals(lane, grid))
        .collect();
    let row = Row::Sweep {
        sum: Box::new(truncated),
        spans,
        intervals,
    };
    Ok((row, label))
}

/// The row a closed form with no spectral sum takes: one per addend where they differ, else 4.
fn of_written(form: &ClosedForm, grid: Grid, profile: &Profile) -> Result<Labelled, CollapseError> {
    let Some(addends) = addends::addends(form) else {
        return point(form, grid, profile);
    };
    let mut parts = Vec::with_capacity(addends.len());
    let mut labels: Vec<(Source, Detail)> = Vec::new();
    for addend in &addends {
        match of_term(addend, grid, profile)? {
            (Row::Added(inner), (source, Detail::Added { parts: details })) => {
                parts.extend(inner);
                labels.extend(details.into_iter().map(|d| (source, d)));
            }
            (part, (source, detail)) => {
                let (held, reach) = (width(&part), reach(&part, grid));
                parts.push((part, held, reach));
                labels.push((source, detail));
            }
        }
    }
    if parts
        .iter()
        .all(|(part, ..)| matches!(part, Row::Point { .. }))
    {
        return point(form, grid, profile);
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

fn of_term(form: &ClosedForm, grid: Grid, profile: &Profile) -> Result<Labelled, CollapseError> {
    let Ok(sum) = normalize_closed_form(form) else {
        return of_written(form, grid, profile);
    };
    match of_sum(&sum, grid, profile, &Opaque) {
        Err(nested @ CollapseError::NestedSeries { .. }) => Err(nested),
        Err(_) => of_written(form, grid, profile),
        held => held,
    }
}

fn point(form: &ClosedForm, grid: Grid, profile: &Profile) -> Result<Labelled, CollapseError> {
    let written = ClosedForm {
        body: truncate::written(&form.body, Audible::on(profile, grid))?,
        ..form.clone()
    };
    let tail_db = truncate::dropped_db(&written.body);
    let width = column::width_of(&written.body, &[]).max(1);
    let parts = addends::summed(&written.body).unwrap_or_default();
    let intervals = addends::addend_intervals(&parts, grid);
    let roots: Vec<&sva_formula::Body> = std::iter::once(&written.body)
        .chain(parts.iter().map(|p| &*p.body))
        .collect();
    let programs = (0..width).map(|c| Program::of(&roots, &[], c)).collect();
    let row = Row::Point {
        written: Box::new(written),
        width,
        intervals,
        programs,
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
fn reach(row: &Row, grid: Grid) -> SampleInterval {
    let hull = |spans: &mut dyn Iterator<Item = SampleInterval>| {
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
            written, intervals, ..
        } => match addends::summed(&written.body) {
            Some(_) => hull(&mut intervals.iter().copied()),
            None => addends::live_interval(&written.body, grid),
        },
        Row::Added(parts) => hull(&mut parts.iter().map(|(.., reach)| *reach)),
    }
}

fn values(
    row: &Row,
    c: usize,
    (from, to): SampleInterval,
    grid: Grid,
) -> Result<Vec<f64>, CollapseError> {
    let n = (to - from) as usize;
    Ok(match row {
        Row::Lines(lanes) => (from..to)
            .map(|i| {
                let mut held = 0.0;
                if let Some(direct) = &lanes[c] {
                    held += direct.at(grid.instant(i));
                }
                held
            })
            .collect(),
        // Each group's sum from +0, times its factor.
        Row::Grouped(lanes) => {
            let mut out = vec![0.0; n];
            for g in &lanes[c] {
                let (a, b) = active::meet((from, to), g.live);
                for m in a..b {
                    let summed = g
                        .direct
                        .as_ref()
                        .map_or(0.0, |d| 0.0 + d.at(grid.instant(m)));
                    out[(m - from) as usize] += summed
                        * point::eval_atom_on(&g.factor, grid.instant(m), Some((grid, m)))?.re;
                }
            }
            out
        }
        Row::Sweep {
            sum,
            spans,
            intervals,
        } => {
            let mut out = vec![0.0; n];
            for (a, b) in inside(spans[c].as_deref(), (from, to)) {
                let at = (a - from) as usize..(b - from) as usize;
                active::sweep(&sum.lanes[c], &intervals[c], (a, b), grid, &mut out[at])?;
            }
            out
        }
        Row::Point {
            written,
            intervals,
            programs,
            ..
        } => {
            let summed = addends::summed(&written.body).is_some();
            point_values(&programs[c], summed.then_some(intervals), (from, to), grid)?
        }
        Row::Added(parts) => {
            let mut sum = vec![0.0; n];
            for (part, held, reach) in parts {
                if let (Some(lane), (a, b)) = (lane(*held, c), active::meet((from, to), *reach))
                    && a < b
                {
                    let at = (a - from) as usize;
                    for (out, v) in sum[at..].iter_mut().zip(values(part, lane, (a, b), grid)?) {
                        *out += v;
                    }
                }
            }
            sum
        }
    })
}

/// Lanes a written form is evaluated over at once.
const CHUNK: i64 = 256;

/// Samples `[from, to)` of a written form's component, from the whole form, or, given each
/// addend's interval, summed from +0 in order over the addends live at each sample: one left
/// out is exactly zero there. The first sample refusing refuses them all.
fn point_values(
    program: &Program,
    intervals: Option<&Vec<SampleInterval>>,
    (from, to): SampleInterval,
    grid: Grid,
) -> Result<Vec<f64>, CollapseError> {
    let mut out = vec![0.0; (to - from).max(0) as usize];
    let mut columns = program.columns(CHUNK.min(to - from).max(0) as usize);
    columns.grid = grid;
    let mut at = from;
    while at < to {
        let end = (at + CHUNK).min(to);
        let lanes = (end - at) as usize;
        columns.again();
        for (i, n) in (at..end).enumerate() {
            columns.t[i] = grid.instant(n);
            columns.on[i] = Some(n);
        }
        let written = &mut out[(at - from) as usize..][..lanes];
        match intervals {
            None => {
                let whole = columns.root(0, (0, lanes));
                for (i, v) in written.iter_mut().enumerate() {
                    *v = whole.get(i).map_err(Clone::clone)?.re;
                }
            }
            Some(intervals) => {
                let mut sums: Vec<Result<sva_formula::C64, CollapseError>> =
                    vec![Ok(sva_formula::C64::ZERO); lanes];
                for (k, live) in intervals.iter().enumerate() {
                    let (lo, hi) = active::meet((at, end), *live);
                    let (lo, hi) = ((lo - at).max(0) as usize, (hi - at).max(0) as usize);
                    if lo >= hi {
                        continue;
                    }
                    let addend = columns.root(k + 1, (lo, hi));
                    for (i, sum) in sums.iter_mut().enumerate().take(hi).skip(lo) {
                        if let Some(held) = sum.as_ref().ok().copied() {
                            *sum = addend.get(i).map(|v| held + v).map_err(Clone::clone);
                        }
                    }
                }
                for (v, sum) in written.iter_mut().zip(sums) {
                    *v = match sum? {
                        sum if sum.is_finite() => sum.re,
                        _ => return Err(column::INFINITE),
                    };
                }
            }
        }
        at = end;
    }
    Ok(out)
}

/// `values` at an instant between samples, where no interval of samples skips a term.
fn between(row: &Row, c: usize, t: f64) -> Result<f64, CollapseError> {
    Ok(match row {
        Row::Lines(lanes) => {
            let mut held = 0.0;
            if let Some(direct) = &lanes[c] {
                held += direct.at(t);
            }
            held
        }
        Row::Grouped(lanes) => {
            let mut out = 0.0;
            for g in &lanes[c] {
                let summed = g.direct.as_ref().map_or(0.0, |d| 0.0 + d.at(t));
                out += summed * point::eval_atom_on(&g.factor, t, None)?.re;
            }
            out
        }
        Row::Sweep { sum, .. } => point::eval_spectral_sum(sum, c, t)?.re,
        Row::Point { programs, .. } => {
            let mut columns = programs[c].columns(1);
            columns.t[0] = t;
            columns.root(0, (0, 1)).get(0).map_err(Clone::clone)?.re
        }
        Row::Added(parts) => {
            let mut sum = 0.0;
            for (part, held, _) in parts {
                if let Some(lane) = lane(*held, c) {
                    sum += between(part, lane, t)?;
                }
            }
            sum
        }
    })
}

/// `[from, to)` met with a lane's spans, where every atom of it is windowed.
fn inside(spans: Option<&[SampleInterval]>, (from, to): SampleInterval) -> Vec<SampleInterval> {
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

/// A form no atom sum reaches still has a value at every instant. A nesting past the bound
/// is not one of those: it has a form, and too many terms to expand.
fn reaches_no_atom(e: &CollapseError) -> bool {
    matches!(e, CollapseError::NotEvaluable(_))
}
