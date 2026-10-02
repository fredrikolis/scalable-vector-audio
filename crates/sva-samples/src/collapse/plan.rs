// Concern: which row of the collapse table a form takes, what it costs and its direct sums' bounds | Non-concern: running the row | IO: (&SpectralSum, rate, Extent) -> Plan, flops, bounds

use sva_formula::closed_form::{Part, map_children};
use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::{
    Body, ClosedForm, Lane, Line, Opaque, Reads, SpectralSum, Var, normalize_closed_form,
};

use super::active::{self, Window};
use super::truncate::Audible;
use super::{Extent, atoms, lines, point, truncate};
use crate::Grid;
use crate::error::CollapseError;
use crate::label::Rule;
use crate::profile::Profile;

/// Both routes place a line exactly, so cost decides; the direct one hands a lone line back
/// unchanged.
pub fn direct_is_cheaper(lines: usize, n: usize, len: usize) -> bool {
    (lines as f64) * (len as f64) <= (n as f64) * (n as f64).log2()
}

pub struct LinePlan {
    pub placed: Vec<Vec<Line>>,
    pub summed: Vec<Vec<Line>>,
    pub dropped: Vec<Vec<Line>>,
    pub bins: usize,
    pub tail_db: Option<f64>,
}

/// FORMAT 9.2's window route. Outside `live` the factor zeroes a sum proven finite.
pub struct Group {
    pub factor: SpectralAtom,
    pub placed: Vec<Line>,
    pub summed: Vec<Line>,
    pub live: Window,
    samples: usize,
}

/// `atoms` times `samples` chose the route before a sweep skipped dead atoms, so the choice
/// keeps its bits; `evaluated` is what it now runs.
pub enum LanePlan {
    Grouped {
        groups: Vec<Group>,
        bins: usize,
    },
    Sweep {
        atoms: usize,
        samples: usize,
        evaluated: u128,
    },
}

pub struct Sampled {
    pub sum: Box<SpectralSum>,
    pub rule: Rule,
    pub lanes: Vec<LanePlan>,
}

/// The row of FORMAT 9.1 that runs, decided once for the collapse and the count alike.
pub enum Plan {
    Spectrum(Box<SpectralSum>),
    Lines(Box<LinePlan>),
    Sampled(Box<Sampled>),
    /// Row 4 over the written closed form: every node the truncation leaves, at each instant
    /// it is walked.
    Point {
        written: Box<ClosedForm>,
        width: usize,
    },
    /// A sum whose addends name different rows, each taking its own.
    Added(Vec<Plan>),
}

/// The rows in FORMAT 9.1's order; the first guard that holds decides.
pub fn of(
    sum: &SpectralSum,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
) -> Result<Plan, CollapseError> {
    of_read(sum, (rate, extent, len), profile, &Opaque)
}

pub fn of_read(
    sum: &SpectralSum,
    (rate, extent, len): (u32, Extent, usize),
    profile: &Profile,
    reads: &dyn Reads,
) -> Result<Plan, CollapseError> {
    let ceiling = profile.ceiling(rate);
    if sum.var == Var::F {
        return Ok(Plan::Spectrum(Box::new(sum.clone())));
    }
    if let Some(found) = line_plan(sum, (rate, extent, len), (profile, ceiling), reads)? {
        return Ok(Plan::Lines(Box::new(found)));
    }
    let truncated = truncate::spectral_sum_read(sum, Audible::of(profile, rate), reads)?;
    let rule = match () {
        () if atoms::band_limited(&truncated, ceiling, profile) => Rule::BandLimited,
        () if atoms::windowed(&truncated) => Rule::CroppedPair,
        () => Rule::PointSampled,
    };
    let lanes = truncated
        .lanes
        .iter()
        .map(|lane| lane_plan(lane, rate, extent, len))
        .collect();
    Ok(Plan::Sampled(Box::new(Sampled {
        sum: Box::new(truncated),
        rule,
        lanes,
    })))
}

/// The row a closed form with no spectral sum takes: one per addend where they differ, else 4.
pub fn of_written(
    form: &ClosedForm,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
) -> Result<Plan, CollapseError> {
    let Some(addends) = addends(form) else {
        return point_plan(form, rate, profile);
    };
    let mut parts = Vec::with_capacity(addends.len());
    for addend in &addends {
        match of_term(addend, rate, extent, profile, len)? {
            Plan::Added(inner) => parts.extend(inner),
            part => parts.push(part),
        }
    }
    // Addends that all name row 4 are one row 4 over the whole sum, measured once.
    match parts.iter().all(|part| matches!(part, Plan::Point { .. })) {
        true => point_plan(form, rate, profile),
        false => Ok(Plan::Added(parts)),
    }
}

/// A row the addend alone cannot take refuses nothing; the written row still stands for it.
/// A nesting past the bound is the one exception, refused wherever it is written.
fn of_term(
    form: &ClosedForm,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
) -> Result<Plan, CollapseError> {
    let Ok(sum) = normalize_closed_form(form) else {
        return of_written(form, rate, extent, profile, len);
    };
    match of(&sum, rate, extent, profile, len) {
        Err(nested @ CollapseError::NestedSeries { .. }) => Err(nested),
        Err(_) => of_written(form, rate, extent, profile, len),
        held => held,
    }
}

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

fn point_plan(form: &ClosedForm, rate: u32, profile: &Profile) -> Result<Plan, CollapseError> {
    let written = ClosedForm {
        body: truncate::written(&form.body, Audible::of(profile, rate))?,
        ..form.clone()
    };
    let width = point::width_of(&written.body, &point::NoRefs).max(1);
    Ok(Plan::Point {
        written: Box::new(written),
        width,
    })
}

/// The nodes every component's evaluation walks over the samples `[from, to)` of `grid`.
pub fn point_flops(f: &Body, grid: Grid, span: Window) -> u128 {
    let width = point::width_of(f, &point::NoRefs).max(1);
    (0..width).map(|c| point_work(f, c, grid, span).0).sum()
}

/// One component's `(nodes, waves)` over the samples `[from, to)` of `grid`, as
/// `point::eval_body` walks them: a run is priced by its Horner steps and turns its lines,
/// every other node is one, and a crop's operand counts only at the instants its window holds.
pub fn point_work(f: &Body, component: usize, grid: Grid, span: Window) -> (u128, u128) {
    let clock = Clock::grid(grid);
    let Some(parts) = summed(f) else {
        return walked(f, component, &clock, span);
    };
    let n = (i128::from(span.1) - i128::from(span.0)).max(0) as u128;
    parts
        .iter()
        .zip(addend_windows(&parts, grid))
        .fold((n, 0), |held, (part, live)| {
            let (priced, waves) = walked(&part.body, component, &clock, active::meet(span, live));
            (held.0 + priced, held.1 + waves)
        })
}

/// A written sum's addends in order, a left-nested `a + b + c` as one list: `point::eval_body`
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

/// Outside its own window an addend is exact zero.
pub(super) fn addend_windows(parts: &[&Part], grid: Grid) -> Vec<Window> {
    parts
        .iter()
        .map(|part| live_window(&part.body, grid))
        .collect()
}

pub(super) fn live_window(f: &Body, grid: Grid) -> Window {
    live(f, &Clock::grid(grid), active::OPEN)
}

/// A crop's open window, met through products and shifts, as `point::eval_body` zeroes them.
fn live(f: &Body, clock: &Clock, span: Window) -> Window {
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

    fn warped(&self) -> Clock {
        Clock {
            grid: self.grid,
            shifts: None,
        }
    }

    /// Where a crop's gain is not zero: inside `[l, r)` less the instants a shoulder opens at.
    fn cropped(&self, span: Window, crop: &Body) -> Window {
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
        let (mut from, mut to) = active::meet(span, active::between(l, r, at));
        let shut = |n: i64| point::crop_gain(at(n), l, r, *rise, *fall) == 0.0;
        while from < to && shut(from) {
            from += 1;
        }
        while from < to && shut(to - 1) {
            to -= 1;
        }
        (from, to)
    }
}

fn walked(f: &Body, component: usize, clock: &Clock, span: Window) -> (u128, u128) {
    let n = (i128::from(span.1) - i128::from(span.0)).max(0) as u128;
    if n == 0 {
        return (0, 0);
    }
    let add = |a: (u128, u128), b: (u128, u128)| (a.0 + b.0, a.1 + b.1);
    let branch = |part: &Part, c: usize, clock: &Clock, span: Window| {
        add((n, 0), walked(&part.body, c, clock, span))
    };
    match f {
        Body::Run(run) => (super::run::steps(run) as u128 * n, run.len() as u128 * n),
        Body::Join(parts) => {
            let widths: Vec<usize> = parts
                .iter()
                .map(|p| point::width_of(&p.body, &point::NoRefs))
                .collect();
            match point::lane_of(&widths, component) {
                Some((at, inner)) => branch(&parts[at], inner, clock, span),
                None => (n, 0),
            }
        }
        Body::Channel(of, k) => branch(of, usize::from(*k), clock, span),
        Body::Crop { of, .. } => branch(of, component, clock, clock.cropped(span, f)),
        // `point::product` walks no factor past one a shut crop zeroed.
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

/// Each direct sum a row may take over this form's lines, as its rounding bound and the
/// factor it is read under: none on a line row, the group's own on a windowed one.
pub fn summed_bounds(
    sum: &SpectralSum,
    (profile, rate): (&Profile, u32),
    reads: &dyn Reads,
) -> Result<Vec<(Option<SpectralAtom>, f64)>, CollapseError> {
    if sum.var == Var::F {
        return Ok(Vec::new());
    }
    if let Some(found) = kept_lines(sum, profile, profile.ceiling(rate), reads)? {
        let direct = found.kept.iter().filter_map(|kept| lines::Direct::of(kept));
        return Ok(direct.map(|d| (None, d.bound())).collect());
    }
    let band = Audible::of(profile, rate);
    let groups = sum
        .lanes
        .iter()
        .map(|lane| lines::line_groups(lane, band, reads));
    let Some(groups) = groups.collect::<Option<Vec<_>>>() else {
        return Ok(Vec::new());
    };
    Ok(groups
        .into_iter()
        .flatten()
        .filter_map(|(factor, held)| Some((Some(factor), lines::Direct::of(&held)?.bound())))
        .collect())
}

/// Rows one and two: every atom a line, none of them windowed.
fn line_plan(
    sum: &SpectralSum,
    (rate, extent, len): (u32, Extent, usize),
    (profile, ceiling): (&Profile, f64),
    reads: &dyn Reads,
) -> Result<Option<LinePlan>, CollapseError> {
    let Some(found) = kept_lines(sum, profile, ceiling, reads)? else {
        return Ok(None);
    };
    let bins = lines::grid(&found.kept, &found.grids, rate, extent.span_secs(rate), len);
    let split: Vec<(Vec<Line>, Vec<Line>)> = found
        .kept
        .iter()
        .map(|k| match direct_is_cheaper(k.len(), bins, len) {
            true => (Vec::new(), k.clone()),
            false => lines::split(k, bins, rate),
        })
        .collect();
    Ok(Some(LinePlan {
        placed: split.iter().map(|(on, _)| on.clone()).collect(),
        summed: split.into_iter().map(|(_, off)| off).collect(),
        dropped: found.dropped,
        bins,
        tail_db: found.tail,
    }))
}

/// Each lane's lines under the ceiling and over it, before any extent places them.
pub(super) struct Kept {
    pub(super) kept: Vec<Vec<Line>>,
    dropped: Vec<Vec<Line>>,
    grids: Vec<f64>,
    tail: Option<f64>,
}

impl Kept {
    pub(super) fn dropped(&self) -> &[Vec<Line>] {
        &self.dropped
    }

    pub(super) fn tail(&self) -> Option<f64> {
        self.tail
    }
}

pub(super) fn kept_lines(
    sum: &SpectralSum,
    profile: &Profile,
    ceiling: f64,
    reads: &dyn Reads,
) -> Result<Option<Kept>, CollapseError> {
    let mut per_lane = Vec::with_capacity(sum.lanes.len());
    let mut grids: Vec<f64> = Vec::new();
    let mut tail: Option<f64> = None;
    for lane in &sum.lanes {
        let band = (ceiling, profile.floor(ceiling), profile.half_lsb());
        match lines::of_lane(lane, band, reads) {
            Some(found) => {
                if let Some(left) = found.tail_db {
                    tail = Some(tail.map_or(left, |held: f64| held.max(left)));
                }
                grids.extend(found.grids);
                per_lane.push(found.lines);
            }
            None => return Ok(None),
        }
    }

    let (mut kept, mut dropped): (Vec<Vec<Line>>, Vec<Vec<Line>>) = (Vec::new(), Vec::new());
    for found in &per_lane {
        let (here, gone): (Vec<Line>, Vec<Line>) = found.iter().partition(|l| l.hz.abs() < ceiling);
        kept.push(here);
        dropped.push(gone);
    }
    // A form with no line at all is silence, not a band every line sat above.
    if kept.iter().all(Vec::is_empty) && !dropped.iter().all(Vec::is_empty) {
        let lowest = dropped
            .iter()
            .flatten()
            .map(|l| l.hz.abs())
            .fold(f64::INFINITY, f64::min);
        return Err(CollapseError::EmptyBand { ceiling, lowest });
    }
    Ok(Some(Kept {
        kept,
        dropped,
        grids,
        tail,
    }))
}

fn lane_plan(lane: &Lane, rate: u32, extent: Extent, len: usize) -> LanePlan {
    let bins = lines::bins(extent.span_secs(rate), rate);
    let grid = Grid::of(rate);
    let spans = super::span::absolute(lane, extent, rate);
    let sweep = LanePlan::Sweep {
        atoms: lane.atoms.len(),
        samples: spans.iter().map(|(from, to)| (to - from) as usize).sum(),
        evaluated: active::evaluated(&active::windows(lane, grid), &spans),
    };
    let Some(found) = lines::grouped(lane) else {
        return sweep;
    };
    let groups: Vec<Group> = found
        .into_iter()
        .map(|(factor, held)| {
            let (placed, summed) = match direct_is_cheaper(held.len(), bins, len) {
                true => (Vec::new(), held),
                false => lines::split(&held, bins, rate),
            };
            let live = group_window(&factor, &[placed.as_slice(), &summed].concat(), bins, grid);
            Group {
                samples: active::evaluated(&[live], &[(extent.start, extent.end)]) as usize,
                factor,
                placed,
                summed,
                live,
            }
        })
        .collect();
    let grouped = LanePlan::Grouped { groups, bins };
    match grouped.chosen_by(len) <= sweep.chosen_by(len) {
        true => grouped,
        false => sweep,
    }
}

/// Outside it the factor zeroes its lines' sum, where that sum, `bins` times over, is finite.
pub(super) fn group_window(
    factor: &SpectralAtom,
    held: &[Line],
    bins: usize,
    grid: Grid,
) -> Window {
    let reach: f64 = held.iter().map(|l| l.amp.re.abs() + l.amp.im.abs()).sum();
    match reach * (bins.max(1) as f64) < 1e300 {
        true => active::window(factor, grid),
        false => active::OPEN,
    }
}

/// `n*log2(n)` butterflies; a length that is no power of two is Bluestein's three
/// transforms over the next one past `2n-1`.
pub fn transform_flops(n: usize) -> u128 {
    let stages = |m: usize| m as u128 * (m.max(2).trailing_zeros() as u128);
    match n.is_power_of_two() {
        true => stages(n),
        false => 3 * stages((2 * n - 1).next_power_of_two()),
    }
}

impl LinePlan {
    pub fn flops(&self, len: usize) -> u128 {
        let transforms: u128 = self
            .placed
            .iter()
            .filter(|p| !p.is_empty())
            .map(|_| transform_flops(self.bins))
            .sum();
        let direct: u128 = self
            .summed
            .iter()
            .map(|s| s.len() as u128 * len as u128)
            .sum();
        transforms + direct
    }

    pub fn rule(&self) -> Rule {
        match (lines::distinct(&self.placed), lines::distinct(&self.summed)) {
            (_, 0) => Rule::LineSpectrumExact,
            (0, _) => Rule::LineSpectrumSummed,
            _ => Rule::LineSpectrumMixed,
        }
    }
}

impl LanePlan {
    pub fn flops(&self) -> u128 {
        match self {
            LanePlan::Grouped { groups, bins } => groups
                .iter()
                .map(|g| transformed(g, *bins) + (g.summed.len() as u128 + 1) * g.samples as u128)
                .sum(),
            LanePlan::Sweep { evaluated, .. } => *evaluated,
        }
    }

    fn chosen_by(&self, len: usize) -> u128 {
        match self {
            LanePlan::Grouped { groups, bins } => groups
                .iter()
                .map(|g| transformed(g, *bins) + (g.summed.len() as u128 + 1) * len as u128)
                .sum(),
            LanePlan::Sweep { atoms, samples, .. } => *atoms as u128 * *samples as u128,
        }
    }

    fn swept_flops(&self, len: u128) -> u128 {
        let terms: u128 = match self {
            LanePlan::Grouped { groups, .. } => groups
                .iter()
                .map(|g| (g.placed.len() + g.summed.len() + 1) as u128)
                .sum(),
            LanePlan::Sweep { atoms, .. } => *atoms as u128,
        };
        terms * len
    }
}

fn transformed(g: &Group, bins: usize) -> u128 {
    match g.placed.is_empty() {
        true => 0,
        false => transform_flops(bins),
    }
}

impl Plan {
    /// Outside it every sample is +0.0 and inside each is its own instant's, so a run over it
    /// alone holds the same bits; a row reading the whole extent at once answers all of it.
    pub fn nonzero(&self, rate: u32, extent: Extent) -> Extent {
        let grid = Grid::of(rate);
        let lane = |lane: &Lane, taken: &LanePlan| -> Option<Option<Window>> {
            match taken {
                LanePlan::Sweep { .. } if lane.modal.is_empty() => Some(active::hull(
                    &active::windows(lane, grid),
                    &super::span::absolute(lane, extent, rate),
                )),
                LanePlan::Grouped { groups, .. } if groups.iter().all(|g| g.placed.is_empty()) => {
                    Some(active::hull(
                        &groups.iter().map(|g| g.live).collect::<Vec<_>>(),
                        &[(extent.start, extent.end)],
                    ))
                }
                _ => None,
            }
        };
        let found = match self {
            Plan::Sampled(held) => held
                .sum
                .lanes
                .iter()
                .zip(&held.lanes)
                .map(|(l, taken)| lane(l, taken))
                .collect::<Option<Vec<_>>>()
                .map(|lanes| lanes.into_iter().flatten().collect::<Vec<_>>()),
            Plan::Added(parts) => Some(
                parts
                    .iter()
                    .map(|part| part.nonzero(rate, extent))
                    .filter(|part| !part.is_empty())
                    .map(|part| (part.start, part.end))
                    .collect(),
            ),
            _ => None,
        };
        match found {
            None => extent,
            Some(spans) => spans
                .into_iter()
                .map(|(a, b)| Extent::new(a, b))
                .fold(Extent::NOWHERE, Extent::hull)
                .intersect(extent),
        }
    }

    /// What two extents cut to one may still differ in.
    pub fn route(&self) -> Vec<u64> {
        let lines = |held: &[Vec<Line>]| held.iter().map(|l| l.len() as u64).collect::<Vec<_>>();
        match self {
            Plan::Spectrum(_) => vec![0],
            Plan::Lines(found) => [vec![1], lines(&found.placed), lines(&found.summed)].concat(),
            Plan::Sampled(held) => {
                let mut out = vec![2, held.rule as u64];
                for lane in &held.lanes {
                    match lane {
                        LanePlan::Sweep { .. } => out.push(0),
                        LanePlan::Grouped { groups, .. } => {
                            out.push(1 + groups.len() as u64);
                            for g in groups {
                                out.extend([g.placed.len() as u64, g.summed.len() as u64]);
                            }
                        }
                    }
                }
                out
            }
            Plan::Point { .. } => vec![3],
            Plan::Added(parts) => {
                let mut out = vec![4, parts.len() as u64];
                parts.iter().for_each(|part| out.extend(part.route()));
                out
            }
        }
    }

    pub fn flops(&self, rate: u32, extent: Extent) -> u128 {
        let len = extent.len();
        match self {
            Plan::Spectrum(_) => transform_flops(len),
            Plan::Lines(found) => found.flops(len),
            Plan::Sampled(held) => held.lanes.iter().map(|lane| lane.flops()).sum(),
            Plan::Point { written, .. } => {
                point_flops(&written.body, Grid::of(rate), (extent.start, extent.end))
            }
            Plan::Added(parts) => parts.iter().map(|part| part.flops(rate, extent)).sum(),
        }
    }

    pub fn alias_flops(&self, rate: u32, extent: Extent) -> u128 {
        self.alias_flops_at(rate, extent, super::ALIAS_OVERSAMPLE)
    }

    /// The multiple a reading names need not be the label's `ALIAS_OVERSAMPLE`. Every component
    /// is scored, so every component's reference is paid for.
    pub fn alias_flops_at(&self, rate: u32, extent: Extent, oversample: usize) -> u128 {
        let reference = (extent.len() * oversample) as u128;
        match self {
            Plan::Point { written, .. } => {
                let finer = oversample as i64;
                point_flops(
                    &written.body,
                    Grid::finer(rate, oversample),
                    (extent.start * finer, extent.end * finer),
                )
            }
            Plan::Sampled(held) if held.rule == Rule::PointSampled => held
                .lanes
                .iter()
                .map(|lane| lane.swept_flops(reference))
                .sum(),
            Plan::Added(parts) => parts
                .iter()
                .map(|part| part.alias_flops_at(rate, extent, oversample))
                .sum(),
            Plan::Spectrum(_) | Plan::Lines(_) | Plan::Sampled(_) => 0,
        }
    }

    pub fn rule(&self) -> Rule {
        match self {
            Plan::Spectrum(_) => Rule::InverseSpectrum,
            Plan::Lines(found) => found.rule(),
            Plan::Sampled(held) => held.rule,
            Plan::Point { .. } => Rule::PointSampled,
            Plan::Added(_) => Rule::Added,
        }
    }
}
