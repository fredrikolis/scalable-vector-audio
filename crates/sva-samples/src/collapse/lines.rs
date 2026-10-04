// Concern: a sum's lines under the ceiling, grouped by common factor, summed at an instant, bounded | Non-concern: which row takes them (blocks.rs) | IO: (&SpectralSum, ceiling) -> lines, Direct, bounds

use std::f64::consts::TAU;

use std::collections::BTreeMap;

use sva_formula::spectral_sum::atom::{Exp, Factors, Singular, SpectralAtom, SpectralAtomKey};
use sva_formula::{C64, Lane, Line, Origin, Reads, Run, SpectralSum, Var, lines_read, modal};

use super::active::{self, Window};
use super::truncate::Audible;
use crate::Grid;
use crate::error::CollapseError;
use crate::label::Dropped;
use crate::profile::Profile;

struct Found {
    lines: Vec<Line>,
    tail_db: Option<f64>,
}

fn of_lane(
    lane: &Lane,
    (ceiling, floor_db, precision): (f64, f64, f64),
    reads: &dyn Reads,
) -> Option<Found> {
    let mut out = Vec::new();
    let mut tail: Option<f64> = None;
    for atom in &lane.atoms {
        out.push(as_line(atom)?);
    }
    for bank in &lane.modal {
        for atom in modal::atoms(bank, Origin::UNKNOWN) {
            out.push(as_line(&atom)?);
        }
    }
    for series in &lane.series {
        let enumerated = lines_read(series, (ceiling, floor_db, precision), reads)?;
        if enumerated.taken.is_empty() && enumerated.dropped.is_empty() {
            return None;
        }
        if enumerated.tail_db.is_finite() {
            tail = Some(tail.map_or(enumerated.tail_db, |held: f64| held.max(enumerated.tail_db)));
        }
        out.extend(enumerated.taken);
        out.extend(enumerated.dropped);
    }
    Some(Found {
        lines: out,
        tail_db: tail,
    })
}

/// Each direct sum a row takes over this form's lines, as its rounding bound and the factor it
/// is read under: none on a line row, the group's own on a windowed one.
pub fn summed_bounds(
    sum: &SpectralSum,
    (profile, rate): (&Profile, u32),
    reads: &dyn Reads,
) -> Result<Vec<(Option<SpectralAtom>, f64)>, CollapseError> {
    if sum.var == Var::F {
        return Ok(Vec::new());
    }
    if let Some(found) = kept_lines(sum, profile, profile.ceiling(rate), reads)? {
        let direct = found.kept.iter().filter_map(|kept| Direct::of(kept));
        return Ok(direct.map(|d| (None, d.bound())).collect());
    }
    let band = Audible::of(profile, rate);
    let groups = sum.lanes.iter().map(|lane| line_groups(lane, band, reads));
    let Some(groups) = groups.collect::<Option<Vec<_>>>() else {
        return Ok(Vec::new());
    };
    Ok(groups
        .into_iter()
        .flatten()
        .filter_map(|(factor, held)| Some((Some(factor), Direct::of(&held)?.bound())))
        .collect())
}

/// Each lane's lines under the ceiling and over it.
pub(super) struct Kept {
    pub(super) kept: Vec<Vec<Line>>,
    dropped: Vec<Vec<Line>>,
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
    let mut tail: Option<f64> = None;
    for lane in &sum.lanes {
        let band = (ceiling, profile.floor(ceiling), profile.half_lsb());
        match of_lane(lane, band, reads) {
            Some(found) => {
                if let Some(left) = found.tail_db {
                    tail = Some(tail.map_or(left, |held: f64| held.max(left)));
                }
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
        return Err(CollapseError::EmptyBand {
            ceiling,
            lowest,
            by_rate: ceiling < profile.ceiling_hz,
        });
    }
    Ok(Some(Kept {
        kept,
        dropped,
        tail,
    }))
}

/// Outside it the factor zeroes its lines' sum, where that sum is finite.
pub(super) fn group_window(factor: &SpectralAtom, held: &[Line], grid: Grid) -> Window {
    let reach: f64 = held.iter().map(|l| l.amp.re.abs() + l.amp.im.abs()).sum();
    match reach < 1e300 {
        true => active::window(factor, grid),
        false => active::OPEN,
    }
}

/// `c * exp(i*omega*t)` and nothing else; every other factor is another row.
fn as_line(a: &SpectralAtom) -> Option<Line> {
    if !a.poly.is_one()
        || a.gauss.is_some()
        || a.ind.is_some()
        || a.pole.is_some()
        || !matches!(a.sing, Singular::Regular)
    {
        return None;
    }
    let omega = a
        .exp
        .map_or(0.0, |e| if e.sigma == 0.0 { e.omega } else { f64::NAN });
    omega.is_finite().then(|| Line::bare(omega / TAU, a.c))
}

/// A lane's atoms as line spectra under their common real factors, one group per factor.
fn grouped(lane: &Lane) -> Option<Vec<(SpectralAtom, Vec<Line>)>> {
    if !lane.is_finite_sum() || lane.atoms.is_empty() {
        return None;
    }
    let mut at: BTreeMap<SpectralAtomKey, usize> = BTreeMap::new();
    let mut out: Vec<(SpectralAtom, Vec<Line>)> = Vec::new();
    for a in &lane.atoms {
        let exp = a.exp.unwrap_or(Exp::at(0.0, 0.0));
        if exp.sigma != 0.0 || a.pole.is_some() || !matches!(a.sing, Singular::Regular) {
            return None;
        }
        let held = *at.entry(a.key_without_line()).or_insert_with(|| {
            out.push((
                SpectralAtom {
                    c: C64::ONE,
                    exp: None,
                    ..*a
                },
                Vec::new(),
            ));
            out.len() - 1
        });
        out[held].1.push(Line::bare(exp.omega / TAU, a.c));
    }
    Some(out)
}

/// A lane's lines under each common factor, each series' on its own ladders under its window.
pub(super) fn line_groups(
    lane: &Lane,
    band: super::truncate::Audible,
    reads: &dyn Reads,
) -> Option<Vec<(SpectralAtom, Vec<Line>)>> {
    if !lane.modal.is_empty() {
        return None;
    }
    let atoms = Lane {
        series: Vec::new(),
        ..lane.clone()
    };
    let mut out = match atoms.atoms.is_empty() {
        true => Vec::new(),
        false => grouped(&atoms)?,
    };
    out.extend(ladders(lane, band, reads)?);
    Some(out)
}

fn ladders(
    lane: &Lane,
    band: super::truncate::Audible,
    reads: &dyn Reads,
) -> Option<Vec<(SpectralAtom, Vec<Line>)>> {
    lane.series
        .iter()
        .map(|series| {
            let (held, ind) = super::truncate::windowed_lines(series, band, reads)?;
            let factors = Factors {
                ind,
                ..Factors::NONE
            };
            let factor =
                SpectralAtom::new(C64::ONE, factors, Singular::Regular, series.term.origin);
            Some((factor, held))
        })
        .collect()
}

/// Kept lines summed at one instant: the constant lines folded to one level, the rest as runs.
pub(crate) struct Direct {
    level: f64,
    runs: Vec<Run>,
    lines: usize,
}

impl Direct {
    pub(crate) fn of(kept: &[Line]) -> Option<Direct> {
        if kept.is_empty() {
            return None;
        }
        let (dc, moving): (Vec<Line>, Vec<Line>) = kept.iter().partition(|l| l.hz == 0.0);
        Some(Direct {
            level: dc.iter().map(|l| l.amp.re).sum(),
            runs: Run::of(&moving),
            lines: kept.len(),
        })
    }

    /// Every moving line's frequency.
    pub(crate) fn hz(&self) -> Vec<f64> {
        self.runs
            .iter()
            .flat_map(Run::lines)
            .map(|l| l.hz)
            .collect()
    }

    pub(crate) fn at(&self, t: f64) -> f64 {
        let moving: f64 = self.runs.iter().map(|r| super::run::at(r, t).re).sum();
        self.level + moving
    }

    pub(crate) fn lines_priced_and_turned(&self) -> (usize, usize) {
        (self.lines, self.runs.iter().map(Run::len).sum())
    }

    /// How far `at` sits from the exact sum at any instant.
    pub(crate) fn bound(&self) -> f64 {
        let runs: f64 = self.runs.iter().map(super::run::bound).sum();
        let reach: f64 = self.runs.iter().map(super::run::reach).sum::<f64>() + self.level.abs();
        let adds = (self.runs.len() + 2) as f64 * f64::EPSILON;
        (runs + adds * reach) * (1.0 + adds)
    }
}

/// Ascending by frequency, the 64 loudest kept; `db` against amplitude 1.0, unclamped.
/// Across lanes the loudest channel reports, never the sum of the channels.
pub(crate) fn dropped_list(per_lane: &[Vec<Line>]) -> (Vec<Dropped>, usize) {
    let mut folded = merge(per_lane, f64::max);
    let total = folded.len();
    if total > 64 {
        folded.sort_by(|a, b| b.1.total_cmp(&a.1));
        folded.truncate(64);
        folded.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let list = folded
        .into_iter()
        .map(|(hz, amp)| Dropped {
            hz,
            db: 20.0 * amp.log10(),
        })
        .collect();
    (list, total.saturating_sub(64))
}

pub(crate) fn distinct(per_lane: &[Vec<Line>]) -> usize {
    merge(per_lane, f64::max).len()
}

/// `across` settles what two channels carrying one line report.
fn merge(per_lane: &[Vec<Line>], across: fn(f64, f64) -> f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::new();
    for lane in per_lane {
        out.extend(fold_lane(lane));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::with_capacity(out.len());
    for (hz, amp) in out {
        match merged.last_mut() {
            Some((at, held)) if *at == hz => *held = across(*held, amp),
            _ => merged.push((hz, amp)),
        }
    }
    merged
}

/// `+f` and `-f` fold into the peak amplitude the pair reaches.
fn fold_lane(lane: &[Line]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = lane.iter().map(|l| (l.hz.abs(), l.amp.abs())).collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut folded: Vec<(f64, f64)> = Vec::with_capacity(out.len());
    for (hz, amp) in out {
        match folded.last_mut() {
            Some((at, held)) if *at == hz => *held += amp,
            _ => folded.push((hz, amp)),
        }
    }
    folded
}
