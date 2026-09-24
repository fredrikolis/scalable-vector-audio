// Concern: turns a closed form into samples by the rule its shape names | Non-concern: its spectral sum (sva-formula), reading the buffer (measure/) | IO: (&ClosedForm, rate) -> Buffer, Label

mod atoms;
mod inverse;
mod lines;
pub mod plan;
mod point;
mod reading;
mod span;
mod sum;
mod tail;
mod truncate;

use sva_formula::{Body, C64, ClosedForm, SpectralSum, Var, normalize_closed_form};

use crate::buffer::Buffer;
use crate::error::CollapseError;
use crate::label::{Detail, Label, Rule, Source};
use crate::profile::Profile;

pub use plan::transform_flops;

use plan::Plan;
pub use point::{Refs, crop_gain, lane_of, unary};
pub use truncate::{Audible, spectral_sum as truncate_spectral_sum, written as truncate_written};

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Horizon {
    pub start_secs: f64,
    pub end_secs: f64,
}

impl Horizon {
    pub fn secs(start_secs: f64, end_secs: f64) -> Horizon {
        Horizon {
            start_secs,
            end_secs,
        }
    }

    pub fn span(&self) -> f64 {
        self.end_secs - self.start_secs
    }

    /// An observation that named no window has no horizon at all, and no collapse can run
    /// against one.
    pub fn len(&self, rate: u32) -> Result<usize, CollapseError> {
        let span = self.span();
        if !span.is_finite() || span <= 0.0 {
            return Err(CollapseError::NoHorizon);
        }
        Ok(((span * f64::from(rate)).round() as usize).max(1))
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
    horizon: Horizon,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    match normalize_closed_form(form) {
        Ok(sum) => of_spectral_sum_or_point(&sum, Some(form), rate, horizon, profile, score),
        Err(_) if form.var == Var::T => render_written(form, rate, horizon, profile, score),
        Err(left) => Err(CollapseError::LeftAlgebra(left.reason.clause())),
    }
}

/// FORMAT 9.1's row 4, or one row per addend where a sum's addends name different ones.
pub fn render_written(
    form: &ClosedForm,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let len = horizon.len(rate)?;
    run(
        plan::of_written(form, rate, horizon, profile, len)?,
        rate,
        horizon,
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
    horizon: Horizon,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let planes = (0..width)
        .map(|c| reading::sampled_body(written, c, rate, horizon, len, 1.0))
        .collect::<Result<Vec<_>, _>>()?;
    let alias_db = match score {
        AliasScore::Asked => Some(formula_alias(written, rate, horizon, len, &planes)?),
        AliasScore::NotAsked => None,
    };
    Ok(measured(
        planes,
        rate,
        horizon,
        profile,
        Detail::Point {
            rule: Rule::PointSampled,
            alias_db,
        },
    ))
}

/// The six rows over a spectral sum, falling to the point-sampled row over the written closed
/// form where no atom sum bounds the series.
pub fn of_spectral_sum_or_point(
    sum: &SpectralSum,
    written: Option<&ClosedForm>,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    match of_spectral_sum(sum, rate, horizon, profile, score) {
        Err(e) if reaches_no_atom(&e) => match written.filter(|t| t.var == Var::T) {
            Some(form) => render_written(form, rate, horizon, profile, score),
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
    horizon: Horizon,
    profile: &Profile,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let len = horizon.len(rate)?;
    of_sum(sum, rate, horizon, profile, len, score)
}

/// The row `plan` named, run: one decision, and this is the half that makes samples.
fn of_sum(
    sum: &SpectralSum,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    run(
        plan::of(sum, rate, horizon, profile, len)?,
        rate,
        horizon,
        profile,
        len,
        score,
    )
}

fn run(
    plan: Plan,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    match plan {
        Plan::Spectrum(sum) => spectrum_row(&sum, rate, horizon, profile, len),
        Plan::Lines(found) => Ok(line_row(&found, rate, horizon, profile, len)),
        Plan::Sampled(held) => sampled_row(&held, rate, horizon, profile, len, score),
        Plan::Point { written, width, .. } => {
            point_row(&written, width, rate, horizon, profile, len, score)
        }
        Plan::Added(parts) => sum::added(parts, rate, horizon, profile, len, score),
    }
}

fn line_row(
    found: &plan::LinePlan,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
) -> (Buffer, Label) {
    let planes: Vec<Vec<f64>> = found
        .placed
        .iter()
        .zip(&found.summed)
        .map(|(placed, summed)| {
            let mut plane = lines::transformed(placed, horizon.start_secs, found.bins, rate, len);
            lines::add_direct(&mut plane, summed, horizon.start_secs, rate);
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
    exact(planes, rate, horizon, profile, detail)
}

fn sampled_row(
    held: &plan::Sampled,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
    score: AliasScore,
) -> Result<(Buffer, Label), CollapseError> {
    let (sum, rule) = (&held.sum, held.rule);
    let planes = reading::sampled_spectral_sum(sum, &held.lanes, rate, horizon, len)?;
    match rule {
        Rule::BandLimited => Ok(exact(
            planes,
            rate,
            horizon,
            profile,
            Detail::Continuous { rule },
        )),
        Rule::CroppedPair => Ok(measured(
            planes,
            rate,
            horizon,
            profile,
            Detail::Cropped {
                rule,
                tail_db: tail::tail_db(sum, profile.ceiling(rate)),
            },
        )),
        _ => {
            let alias_db = match score {
                AliasScore::Asked => Some(spectral_sum_alias(sum, rate, horizon, len, &planes)?),
                AliasScore::NotAsked => None,
            };
            Ok(measured(
                planes,
                rate,
                horizon,
                profile,
                Detail::Point { rule, alias_db },
            ))
        }
    }
}

fn spectrum_row(
    sum: &SpectralSum,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    len: usize,
) -> Result<(Buffer, Label), CollapseError> {
    let mut planes = Vec::with_capacity(sum.lanes.len());
    for c in 0..sum.lanes.len() {
        planes.push(inverse::collapse_lane(
            sum,
            c,
            horizon.start_secs,
            horizon.span(),
            rate,
            len,
        )?);
    }
    let wrap_db = inverse::wrap_db(sum, horizon.span(), rate)?;
    Ok(measured(
        planes,
        rate,
        horizon,
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
    horizon: Horizon,
    len: usize,
    planes: &[Vec<f64>],
) -> Result<f64, CollapseError> {
    let mut scored = Vec::with_capacity(planes.len());
    for (c, base) in planes.iter().enumerate() {
        let high = reading::sampled_body(
            form,
            c,
            rate,
            horizon,
            len * ALIAS_OVERSAMPLE,
            ALIAS_OVERSAMPLE as f64,
        )?;
        scored.push(one_component(base, &high, rate, horizon));
    }
    Ok(worst_of(scored))
}

fn spectral_sum_alias(
    sum: &SpectralSum,
    rate: u32,
    horizon: Horizon,
    len: usize,
    planes: &[Vec<f64>],
) -> Result<f64, CollapseError> {
    let step = 1.0 / (f64::from(rate) * ALIAS_OVERSAMPLE as f64);
    let mut scored = Vec::with_capacity(planes.len());
    for (c, base) in planes.iter().enumerate() {
        let high: Vec<f64> = (0..len * ALIAS_OVERSAMPLE)
            .map(|i| {
                point::eval_spectral_sum(sum, c, horizon.start_secs + i as f64 * step).map(|v| v.re)
            })
            .collect::<Result<_, _>>()?;
        scored.push(one_component(base, &high, rate, horizon));
    }
    Ok(worst_of(scored))
}

fn one_component(
    base: &[f64],
    high: &[f64],
    rate: u32,
    horizon: Horizon,
) -> crate::measure::alias::Alias {
    crate::measure::alias::measure_alias(
        base,
        high,
        ALIAS_OVERSAMPLE,
        f64::from(rate),
        horizon.start_secs,
    )
}

/// The label carries one number for the node, and `worst` says which component it comes from.
fn worst_of(scored: Vec<crate::measure::alias::Alias>) -> f64 {
    crate::measure::alias::worst(scored).map_or(f64::NEG_INFINITY, |held| held.asr_db)
}

fn exact(
    planes: Vec<Vec<f64>>,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    detail: Detail,
) -> (Buffer, Label) {
    labelled(Source::Exact, planes, rate, horizon, profile, detail)
}

fn measured(
    planes: Vec<Vec<f64>>,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    detail: Detail,
) -> (Buffer, Label) {
    labelled(Source::Measured, planes, rate, horizon, profile, detail)
}

fn labelled(
    source: Source,
    planes: Vec<Vec<f64>>,
    rate: u32,
    horizon: Horizon,
    profile: &Profile,
    detail: Detail,
) -> (Buffer, Label) {
    let mut buffer = Buffer::of_planes(rate, planes);
    buffer.origin_secs = horizon.start_secs;
    (buffer, Label::new(source, profile.name, rate, detail))
}
