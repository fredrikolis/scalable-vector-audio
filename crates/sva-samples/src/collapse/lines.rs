// Concern: places a line spectrum on the grid, transformed or summed | Non-concern: deciding which (collapse.rs) | IO: (&Lane, ceiling) -> Found, planes, bounds

use std::f64::consts::TAU;

use std::collections::BTreeMap;

use sva_formula::spectral_sum::atom::{Exp, Singular, SpectralAtom, SpectralAtomKey};
use sva_formula::{
    C64, Lane, Line, Origin, Run, commensurate, lines as series_lines, modal, spacing,
};

use crate::fft::idft;
use crate::label::Dropped;

pub struct Found {
    pub lines: Vec<Line>,
    pub grids: Vec<f64>,
    pub tail_db: Option<f64>,
}

pub fn of_lane(lane: &Lane, ceiling: f64, floor_db: f64, precision: f64) -> Option<Found> {
    let mut out = Vec::new();
    let mut grids = Vec::new();
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
        let enumerated = series_lines(series, ceiling, floor_db, precision);
        if enumerated.taken.is_empty() && enumerated.dropped.is_empty() {
            return None;
        }
        if enumerated.tail_db.is_finite() {
            tail = Some(tail.map_or(enumerated.tail_db, |held: f64| held.max(enumerated.tail_db)));
        }
        grids.extend(spacing(series));
        out.extend(enumerated.taken);
        out.extend(enumerated.dropped);
    }
    Some(Found {
        lines: out,
        grids,
        tail_db: tail,
    })
}

/// The longest inverse transform one collapse holds; past it the lines are summed.
const MAX_TRANSFORM: usize = 1 << 21;

/// The transform length placing the most lines on bin centres: a series closes over its own
/// spacing, which the horizon need not hold whole.
pub fn grid(kept: &[Vec<Line>], grids: &[f64], rate: u32, span: f64, len: usize) -> usize {
    let mut best = bins(span, rate);
    let mut placed = on_grid(kept, best, rate);
    for step in grids {
        let Some(n) = closing(*step, rate, len) else {
            continue;
        };
        let found = on_grid(kept, n, rate);
        if found > placed || (found == placed && n < best) {
            best = n;
            placed = found;
        }
    }
    best
}

fn closing(step: f64, rate: u32, len: usize) -> Option<usize> {
    let period = f64::from(rate) / step;
    if !period.is_finite() || period <= 0.0 {
        return None;
    }
    let bound = len.max(MAX_TRANSFORM) as f64;
    let base = whole_periods(period, bound)?;
    let n = base * (len as f64 / base).ceil().max(1.0);
    (n <= bound).then_some(n.round() as usize)
}

/// The fewest samples holding whole `period`-sample periods. Only a convergent denominator
/// of `period`'s continued fraction closes it, and they arrive shortest first.
fn whole_periods(period: f64, bound: f64) -> Option<f64> {
    let (mut held, mut before) = (0.0f64, 1.0f64);
    let mut x = period;
    for _ in 0..MAX_CONVERGENTS {
        let whole = x.floor();
        let turns = whole * held + before;
        let samples = period * turns;
        if (samples.round() - samples).abs() <= TURN_EPSILON * samples.max(1.0) {
            return (samples <= bound).then_some(samples.round());
        }
        let fraction = x - whole;
        if samples > bound || fraction <= 0.0 {
            return None;
        }
        x = 1.0 / fraction;
        before = held;
        held = turns;
    }
    None
}

const MAX_CONVERGENTS: usize = 64;

const TURN_EPSILON: f64 = 1e-9;

fn on_grid(kept: &[Vec<Line>], n: usize, rate: u32) -> usize {
    let horizon = n as f64 / f64::from(rate);
    kept.iter()
        .flat_map(|lane| lane.iter())
        .filter(|l| commensurate(l.hz, horizon))
        .count()
}

/// `c * exp(i*omega*t)` and nothing else; every other factor is another row.
fn as_line(a: &SpectralAtom) -> Option<Line> {
    if a.poly > 0
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
pub fn grouped(lane: &Lane) -> Option<Vec<(SpectralAtom, Vec<Line>)>> {
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

/// Kept lines summed at one instant: the constant lines folded to one level, the rest as runs.
pub struct Direct {
    level: f64,
    runs: Vec<Run>,
    lines: usize,
}

impl Direct {
    pub fn of(kept: &[Line]) -> Option<Direct> {
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

    pub fn at(&self, t: f64) -> f64 {
        let moving: f64 = self.runs.iter().map(|r| super::run::at(r, t).re).sum();
        self.level + moving
    }

    pub fn lines_priced_and_turned(&self) -> (usize, usize) {
        (self.lines, self.runs.iter().map(Run::len).sum())
    }

    /// How far `at` sits from the exact sum at any instant.
    pub fn bound(&self) -> f64 {
        let runs: f64 = self.runs.iter().map(super::run::bound).sum();
        let reach: f64 = self.runs.iter().map(super::run::reach).sum::<f64>() + self.level.abs();
        let adds = (self.runs.len() + 2) as f64 * f64::EPSILON;
        (runs + adds * reach) * (1.0 + adds)
    }
}

pub fn add_direct(plane: &mut [f64], kept: &[Line], start_secs: f64, rate: u32) {
    let Some(direct) = Direct::of(kept) else {
        return;
    };
    for (i, held) in plane.iter_mut().enumerate() {
        *held += direct.at(start_secs + i as f64 / f64::from(rate));
    }
}

/// Every kept line falls on a bin centre: no leakage for the exact label to hide.
pub fn transformed(kept: &[Line], start_secs: f64, n: usize, rate: u32, len: usize) -> Vec<f64> {
    if kept.is_empty() {
        return vec![0.0; len];
    }
    let horizon = n as f64 / f64::from(rate);
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for l in kept {
        let bin = (l.hz * horizon).round() as i64;
        let k = bin.rem_euclid(n as i64) as usize;
        let shifted = l.amp * C64::new(0.0, TAU * l.hz * start_secs).exp();
        re[k] += shifted.re * n as f64;
        im[k] += shifted.im * n as f64;
    }
    idft(&mut re, &mut im);
    re.truncate(len);
    re.resize(len, 0.0);
    re
}

pub fn bins(horizon: f64, rate: u32) -> usize {
    (horizon * f64::from(rate)).round().max(1.0) as usize
}

/// DC is never placed: it is a constant fill, not a line the transform rounds.
pub fn split(kept: &[Line], n: usize, rate: u32) -> (Vec<Line>, Vec<Line>) {
    let horizon = n as f64 / f64::from(rate);
    kept.iter()
        .partition(|l| l.hz != 0.0 && commensurate(l.hz, horizon))
}

/// Ascending by frequency, the 64 loudest kept; `db` against amplitude 1.0, unclamped.
/// Across lanes the loudest channel reports, never the sum of the channels.
pub fn dropped_list(per_lane: &[Vec<Line>]) -> (Vec<Dropped>, usize) {
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

pub fn distinct(per_lane: &[Vec<Line>]) -> usize {
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
