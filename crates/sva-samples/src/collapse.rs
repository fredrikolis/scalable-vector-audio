// Concern: turns a closed form into samples by the rule its shape names | Non-concern: its spectral sum (sva-formula), reading the buffer (measure/) | IO: (&ClosedForm, rate) -> Buffer, Label

mod active;
mod atoms;
mod blocks;
mod inverse;
mod lines;
pub mod plan;
mod point;
mod reading;
pub mod run;
mod span;
mod sum;
mod tail;
mod truncate;

use sva_formula::{Body, C64, ClosedForm, Opaque, Reads, SpectralSum, Var, normalize_closed_form};

use crate::Grid;
use crate::buffer::Buffer;
use crate::error::CollapseError;
use crate::label::{Detail, Label, Rule, Source};
use crate::profile::Profile;

pub use blocks::Rows;
pub use plan::transform_flops;

use plan::Plan;
pub(crate) use point::Shared;
pub use point::{Refs, crop_gain, lane_of, shoulders, unary};
pub use truncate::{
    Audible, dropped_db, spectral_sum as truncate_spectral_sum,
    spectral_sum_read as truncate_spectral_sum_read, steady_read as truncate_steady_read,
    written as truncate_written, written_with as truncate_written_with,
};

/// One closed form's value at one instant, for a caller that already holds the spectral sum.
pub fn eval_spectral_sum_at(
    sum: &SpectralSum,
    component: usize,
    t: f64,
) -> Result<C64, CollapseError> {
    point::eval_spectral_sum(sum, component, t)
}

/// One written closed form's value at one instant, every `Body::Node` in it answered by the
/// caller holding the graph.
pub fn eval_written_at(
    body: &Body,
    component: usize,
    t: f64,
    refs: &dyn Refs,
) -> Result<C64, CollapseError> {
    point::eval_body(body, component, t, refs)
}

/// Samples `[start, end)` of the one grid every node is read on, whose sample 0 is t = 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extent {
    pub start: i64,
    pub end: i64,
}

impl Extent {
    /// Every sample of the grid: `i64::MIN` and `i64::MAX` stand for no edge at all.
    pub const EVERYWHERE: Extent = Extent {
        start: i64::MIN,
        end: i64::MAX,
    };

    pub const NOWHERE: Extent = Extent { start: 0, end: 0 };

    pub fn new(start: i64, end: i64) -> Extent {
        assert!(start <= end, "an extent [{start}, {end}) runs backwards");
        Extent { start, end }
    }

    pub fn from(start: i64) -> Extent {
        Extent::new(start, i64::MAX)
    }

    pub fn is_bounded(&self) -> bool {
        self.start != i64::MIN && self.end != i64::MAX
    }

    pub fn contains(&self, n: i64) -> bool {
        self.start <= n && n < self.end
    }

    pub fn intersect(self, other: Extent) -> Extent {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        match start < end {
            true => Extent { start, end },
            false => Extent::NOWHERE,
        }
    }

    pub fn hull(self, other: Extent) -> Extent {
        match (self.is_empty(), other.is_empty()) {
            (true, _) => other,
            (_, true) => self,
            _ => Extent {
                start: self.start.min(other.start),
                end: self.end.max(other.end),
            },
        }
    }

    /// Every sample moved `by` later; an unbounded edge stays unbounded.
    pub fn shifted(self, by: i64) -> Extent {
        if self.is_empty() {
            return self;
        }
        let edge = |n: i64| match n {
            i64::MIN | i64::MAX => n,
            n => n.saturating_add(by).clamp(i64::MIN + 1, i64::MAX - 1),
        };
        Extent {
            start: edge(self.start),
            end: edge(self.end),
        }
    }

    pub fn secs(rate: u32, start_secs: f64, end_secs: f64) -> Extent {
        let at = |secs: f64| (secs * f64::from(rate)).round() as i64;
        Extent::new(at(start_secs), at(end_secs))
    }

    pub fn len(&self) -> usize {
        debug_assert!(self.is_bounded(), "an unbounded extent has no length");
        (self.end - self.start) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }

    pub fn start_secs(&self, rate: u32) -> f64 {
        self.start as f64 / f64::from(rate)
    }

    pub fn span_secs(&self, rate: u32) -> f64 {
        self.len() as f64 / f64::from(rate)
    }
}

/// The oversampling a point-sampled collapse's own alias score is measured against.
pub const ALIAS_OVERSAMPLE: usize = 4;

/// The reference is the same closed form at `ALIAS_OVERSAMPLE` times the rate, so only a reading
/// that reads the score pays for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasScore {
    Asked,
    NotAsked,
}

/// Every row of FORMAT 9.1's table, R0 to R6, by the shape the form names.
pub fn render(
    form: &ClosedForm,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    match normalize_closed_form(form) {
        Ok(sum) => of_spectral_sum_or_point(&sum, Some(form), rate, extent, profile, score),
        Err(_) if form.var == Var::T => render_written(form, rate, extent, profile, score),
        Err(left) => Err(CollapseError::LeftAlgebra(left.reason.clause())),
    }
}

/// FORMAT 9.1's row 4, or one row per addend where a sum's addends name different ones.
pub fn render_written(
    form: &ClosedForm,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let len = extent.len();
    run(
        plan::of_written(form, rate, extent, profile, len)?,
        rate,
        extent,
        profile,
        len,
        score,
    )
}

/// FORMAT 9.1 row 4 over one written closed form: point sampling, measured against its own alias.
fn point_row(
    written: &ClosedForm,
    width: usize,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let planes = (0..width)
        .map(|c| reading::sampled_body(written, c, rate, extent, len, 1))
        .collect::<Result<Vec<_>, _>>()?;
    let alias_db = match score {
        AliasScore::Asked => Some(formula_alias(written, rate, extent, len, &planes)?),
        AliasScore::NotAsked => None,
    };
    Ok(measured(
        planes,
        rate,
        extent,
        profile,
        Detail::Point {
            rule: Rule::PointSampled,
            alias_db,
            tail_db: truncate::dropped_db(&written.body),
        },
    ))
}

/// The six rows over a spectral sum, falling to the point-sampled row over the written closed
/// form where no atom sum bounds the series.
pub fn of_spectral_sum_or_point(
    sum: &SpectralSum,
    written: Option<&ClosedForm>,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    match of_spectral_sum_read(sum, (rate, extent), profile, score, &Opaque) {
        Err(e) if reaches_no_atom(&e) => match written.filter(|t| t.var == Var::T) {
            Some(form) => render_written(form, rate, extent, profile, score),
            None => Err(e),
        },
        other => other,
    }
}

/// A form no atom sum reaches still has a value at every instant. A nesting past the bound
/// is not one of those: it has a form, priced, and too many terms to expand.
pub(crate) fn reaches_no_atom(e: &CollapseError) -> bool {
    matches!(e, CollapseError::NotEvaluable(_))
}

/// The same six rows, for a caller whose refs are already composed in.
pub fn of_spectral_sum(
    sum: &SpectralSum,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    of_spectral_sum_read(sum, (rate, extent), profile, score, &Opaque)
}

pub fn of_spectral_sum_read(
    sum: &SpectralSum,
    (rate, extent): (u32, Extent),
    profile: &Profile,
    score: AliasScore,
    reads: &dyn Reads,
) -> Result<(Buffer, Label), CollapseError> {
    let len = extent.len();
    run(
        plan::of_read(sum, (rate, extent, len), profile, reads)?,
        rate,
        extent,
        profile,
        len,
        score,
    )
}

fn run(
    plan: Plan,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    match plan {
        Plan::Spectrum(sum) => spectrum_row(&sum, rate, extent, profile, len),
        Plan::Lines(found) => Ok(line_row(&found, rate, extent, profile, len)),
        Plan::Sampled(held) => sampled_row(&held, rate, extent, profile, len, score),
        Plan::Point { written, width, .. } => {
            point_row(&written, width, rate, extent, profile, len, score)
        }
        Plan::Added(parts) => sum::added(parts, rate, extent, profile, len, score),
    }
}

fn line_row(
    found: &plan::LinePlan,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
) -> (Buffer, Label) {
    let planes: Vec<Vec<f64>> = found
        .placed
        .iter()
        .zip(&found.summed)
        .map(|(placed, summed)| {
            let mut plane = lines::transformed(placed, extent, found.bins, rate, len);
            lines::add_direct(&mut plane, summed, extent, rate);
            plane
        })
        .collect();
    let (list, more) = lines::dropped_list(&found.dropped);
    let (placed, summed) = (
        lines::distinct(&found.placed),
        lines::distinct(&found.summed),
    );
    let detail = Detail::Lines {
        rule: found.rule(),
        placed,
        summed,
        dropped: list,
        dropped_more: more,
        terms: Some(placed + summed),
        tail_db: found.tail_db,
    };
    exact(planes, rate, extent, profile, detail)
}

fn sampled_row(
    held: &plan::Sampled,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let (sum, rule) = (&held.sum, held.rule);
    let planes = reading::sampled_spectral_sum(sum, &held.lanes, rate, extent, len)?;
    match rule {
        Rule::BandLimited => Ok(exact(
            planes,
            rate,
            extent,
            profile,
            Detail::Continuous { rule },
        )),
        Rule::CroppedPair => Ok(measured(
            planes,
            rate,
            extent,
            profile,
            Detail::Cropped {
                rule,
                tail_db: tail::tail_db(sum, profile.ceiling(rate)),
            },
        )),
        _ => {
            let alias_db = match score {
                AliasScore::Asked => Some(spectral_sum_alias(sum, rate, extent, len, &planes)?),
                AliasScore::NotAsked => None,
            };
            Ok(measured(
                planes,
                rate,
                extent,
                profile,
                Detail::Point {
                    rule,
                    alias_db,
                    tail_db: None,
                },
            ))
        }
    }
}

fn spectrum_row(
    sum: &SpectralSum,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    len: usize,
) -> Result<(Buffer, Label), CollapseError> {
    let mut planes = Vec::with_capacity(sum.lanes.len());
    for c in 0..sum.lanes.len() {
        planes.push(inverse::collapse_lane(
            sum,
            c,
            extent.start_secs(rate),
            extent.span_secs(rate),
            rate,
            len,
        )?);
    }
    let wrap_db = inverse::wrap_db(sum, extent.span_secs(rate), rate)?;
    Ok(measured(
        planes,
        rate,
        extent,
        profile,
        Detail::Spectrum {
            rule: Rule::InverseSpectrum,
            wrap_db,
        },
    ))
}

/// What the grid could not hold is the residual `measure/alias.rs` already knows how to score.
fn formula_alias(
    form: &ClosedForm,
    rate: u32,
    extent: Extent,
    len: usize,
    planes: &[Vec<f64>],
) -> Result<f64, CollapseError> {
    let mut scored = Vec::with_capacity(planes.len());
    for (c, base) in planes.iter().enumerate() {
        let high = reading::sampled_body(
            form,
            c,
            rate,
            extent,
            len * ALIAS_OVERSAMPLE,
            ALIAS_OVERSAMPLE,
        )?;
        scored.push(one_component(base, &high, rate, extent));
    }
    Ok(worst_of(scored))
}

fn spectral_sum_alias(
    sum: &SpectralSum,
    rate: u32,
    extent: Extent,
    len: usize,
    planes: &[Vec<f64>],
) -> Result<f64, CollapseError> {
    let finer = Grid::finer(rate, ALIAS_OVERSAMPLE);
    let from = extent.start * ALIAS_OVERSAMPLE as i64;
    let mut scored = Vec::with_capacity(planes.len());
    for (c, base) in planes.iter().enumerate() {
        let high: Vec<f64> = (0..len * ALIAS_OVERSAMPLE)
            .map(|i| point::eval_spectral_sum(sum, c, finer.instant(from + i as i64)).map(|v| v.re))
            .collect::<Result<_, _>>()?;
        scored.push(one_component(base, &high, rate, extent));
    }
    Ok(worst_of(scored))
}

fn one_component(
    base: &[f64],
    high: &[f64],
    rate: u32,
    extent: Extent,
) -> crate::measure::alias::Alias {
    crate::measure::alias::measure_alias(
        base,
        high,
        ALIAS_OVERSAMPLE,
        f64::from(rate),
        extent.start_secs(rate),
    )
}

/// The label carries one number for the node, and `worst` says which component it comes from.
fn worst_of(scored: Vec<crate::measure::alias::Alias>) -> f64 {
    crate::measure::alias::worst(scored).map_or(f64::NEG_INFINITY, |held| held.asr_db)
}

fn exact(
    planes: Vec<Vec<f64>>,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    detail: Detail,
) -> (Buffer, Label) {
    labelled(Source::Exact, planes, rate, extent, profile, detail)
}

fn measured(
    planes: Vec<Vec<f64>>,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    detail: Detail,
) -> (Buffer, Label) {
    labelled(Source::Measured, planes, rate, extent, profile, detail)
}

fn labelled(
    source: Source,
    planes: Vec<Vec<f64>>,
    rate: u32,
    extent: Extent,
    profile: &Profile,
    detail: Detail,
) -> (Buffer, Label) {
    let mut buffer = Buffer::of_planes(rate, planes);
    buffer.start = extent.start;
    (buffer, Label::new(source, profile.name, rate, detail))
}
