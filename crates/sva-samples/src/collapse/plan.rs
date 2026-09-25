// Concern: which row of the collapse table a form takes, and what that row costs | Non-concern: running the row (collapse.rs) | IO: (&SpectralSum, rate, Horizon) -> Plan, flops

use sva_formula::closed_form::{Part, map_children};
use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::{Body, ClosedForm, Lane, Line, SpectralSum, Var, normalize_closed_form};

use super::truncate::Audible;
use super::{Horizon, atoms, lines, point, truncate};
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

/// FORMAT 9.2's window route.
pub struct Group {
    pub factor: SpectralAtom,
    pub placed: Vec<Line>,
    pub summed: Vec<Line>,
}

pub enum LanePlan {
    Grouped { groups: Vec<Group>, bins: usize },
    Sweep { atoms: usize, samples: usize },
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
    /// Row 4 over the written closed form: every node the truncation leaves, at every instant.
    Point {
        written: Box<ClosedForm>,
        nodes: usize,
        width: usize,
    },
    /// A sum whose addends name different rows, each taking its own.
    Added(Vec<Plan>),
}

/// The rows in FORMAT 9.1's order; the first guard that holds decides.
pub fn of(
    sum: &SpectralSum,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
) -> Result<Plan, CollapseError> {
    let ceiling = profile.ceiling(rate);
    if sum.var == Var::F {
        return Ok(Plan::Spectrum(Box::new(sum.clone())));
    }
    if let Some(found) = line_plan(sum, rate, horizon, profile, len, ceiling)? {
        return Ok(Plan::Lines(Box::new(found)));
    }
    let truncated = truncate::spectral_sum(sum, Audible::of(profile, rate))?;
    let rule = match () {
        () if atoms::band_limited(&truncated, ceiling, profile) => Rule::BandLimited,
        () if atoms::windowed(&truncated) => Rule::CroppedPair,
        () => Rule::PointSampled,
    };
    let lanes = truncated
        .lanes
        .iter()
        .map(|lane| lane_plan(lane, rate, horizon, len))
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
    horizon: Horizon,
    profile: &Profile,
    len: usize,
) -> Result<Plan, CollapseError> {
    let Some(addends) = addends(form) else {
        return point_plan(form, rate, profile);
    };
    let mut parts = Vec::with_capacity(addends.len());
    for addend in &addends {
        match of_term(addend, rate, horizon, profile, len)? {
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
    horizon: Horizon,
    profile: &Profile,
    len: usize,
) -> Result<Plan, CollapseError> {
    let Ok(sum) = normalize_closed_form(form) else {
        return of_written(form, rate, horizon, profile, len);
    };
    match of(&sum, rate, horizon, profile, len) {
        Err(nested @ CollapseError::NestedSeries { .. }) => Err(nested),
        Err(_) => of_written(form, rate, horizon, profile, len),
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
        nodes: point_nodes(&written.body),
        written: Box::new(written),
        width,
    })
}

/// What one instant's evaluation walks over every component: a join walks that component's branch.
pub fn point_nodes(f: &Body) -> usize {
    let width = point::width_of(f, &point::NoRefs).max(1);
    (0..width).map(|c| nodes_at(f, c)).sum()
}

fn nodes_at(f: &Body, component: usize) -> usize {
    let branch = |part: &Part, c: usize| nodes_at(&part.body, c);
    match f {
        Body::Join(parts) => {
            let widths: Vec<usize> = parts
                .iter()
                .map(|p| point::width_of(&p.body, &point::NoRefs))
                .collect();
            match point::lane_of(&widths, component) {
                Some((at, inner)) => 1 + branch(&parts[at], inner),
                None => 1,
            }
        }
        Body::Channel(of, k) => 1 + branch(of, usize::from(*k)),
        Body::Run(run) => super::run::steps(run),
        other => {
            1 + sva_formula::closed_form::children(other)
                .iter()
                .map(|part| branch(part, component))
                .sum::<usize>()
        }
    }
}

/// Rows one and two: every atom a line, none of them windowed.
fn line_plan(
    sum: &SpectralSum,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
    ceiling: f64,
) -> Result<Option<LinePlan>, CollapseError> {
    let Some(found) = kept_lines(sum, profile, ceiling)? else {
        return Ok(None);
    };
    let bins = lines::grid(&found.kept, &found.grids, rate, horizon.span(), len);
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

/// Each lane's lines under the ceiling and over it, before any horizon places them.
pub(super) struct Kept {
    pub(super) kept: Vec<Vec<Line>>,
    dropped: Vec<Vec<Line>>,
    grids: Vec<f64>,
    tail: Option<f64>,
}

pub(super) fn kept_lines(
    sum: &SpectralSum,
    profile: &Profile,
    ceiling: f64,
) -> Result<Option<Kept>, CollapseError> {
    let mut per_lane = Vec::with_capacity(sum.lanes.len());
    let mut grids: Vec<f64> = Vec::new();
    let mut tail: Option<f64> = None;
    for lane in &sum.lanes {
        match lines::of_lane(lane, ceiling, profile.floor(ceiling), profile.half_lsb()) {
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

fn lane_plan(lane: &Lane, rate: u32, horizon: Horizon, len: usize) -> LanePlan {
    let bins = lines::bins(horizon.span(), rate);
    let sweep = LanePlan::Sweep {
        atoms: lane.atoms.len(),
        samples: super::span::nonzero(lane, horizon, rate, len)
            .iter()
            .map(|(from, to)| to - from)
            .sum(),
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
            Group {
                factor,
                placed,
                summed,
            }
        })
        .collect();
    let grouped = LanePlan::Grouped { groups, bins };
    match grouped.flops(len) <= sweep.flops(len) {
        true => grouped,
        false => sweep,
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
    pub fn flops(&self, len: usize) -> u128 {
        match self {
            LanePlan::Grouped { groups, bins } => groups
                .iter()
                .map(|g| {
                    let placed = match g.placed.is_empty() {
                        true => 0,
                        false => transform_flops(*bins),
                    };
                    placed + (g.summed.len() as u128 + 1) * len as u128
                })
                .sum(),
            LanePlan::Sweep { atoms, samples } => *atoms as u128 * *samples as u128,
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

impl Plan {
    pub fn flops(&self, len: usize) -> u128 {
        match self {
            Plan::Spectrum(_) => transform_flops(len),
            Plan::Lines(found) => found.flops(len),
            Plan::Sampled(held) => held.lanes.iter().map(|lane| lane.flops(len)).sum(),
            Plan::Point { nodes, .. } => *nodes as u128 * len as u128,
            Plan::Added(parts) => parts.iter().map(|part| part.flops(len)).sum(),
        }
    }

    pub fn alias_flops(&self, len: usize) -> u128 {
        self.alias_flops_at(len, super::ALIAS_OVERSAMPLE)
    }

    /// The multiple a reading names need not be the label's `ALIAS_OVERSAMPLE`. Every component
    /// is scored, so every component's reference is paid for.
    pub fn alias_flops_at(&self, len: usize, oversample: usize) -> u128 {
        let reference = (len * oversample) as u128;
        match self {
            Plan::Point { nodes, .. } => *nodes as u128 * reference,
            Plan::Sampled(held) if held.rule == Rule::PointSampled => held
                .lanes
                .iter()
                .map(|lane| lane.swept_flops(reference))
                .sum(),
            Plan::Added(parts) => parts
                .iter()
                .map(|part| part.alias_flops_at(len, oversample))
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
