// Concern: decides a series' convergence in A and enumerates the lines it yields | Non-concern: placing them on a grid (sva-samples) | IO: (&Series, ceiling) -> bool, Lines, Enumerated, a spacing

use std::f64::consts::TAU;

use crate::affine::{Axis, Reading, affine_in, axis, exact_constant};
use crate::closed_form::{
    Body, Bound, IndexId, Part, Series, Unary, children, map_children, read_at,
};
use crate::complex::C64;
use crate::env::Env;
use crate::spectral_sum::atom::{Exp, Factors, Singular, SpectralAtom};
use crate::table::series::{Shape, read};

/// A tempered limit needs the coefficient polynomially bounded: `1/k` is, `2^k` is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexGrowth {
    Polynomial,
    Unbounded,
}

pub fn summable(s: &Series, env: &dyn Env) -> bool {
    match s.hi {
        Bound::Finite(_) => true,
        Bound::Infinite => growth(&s.term.body, s.index, env) == IndexGrowth::Polynomial,
    }
}

/// Decided from where the index sits, never by evaluating a term.
pub fn growth(f: &Body, k: IndexId, env: &dyn Env) -> IndexGrowth {
    if !mentions(f, k) {
        return IndexGrowth::Polynomial;
    }
    match f {
        Body::Index(_) | Body::Line | Body::Const(_) => IndexGrowth::Polynomial,
        Body::Add(parts) | Body::Mul(parts) | Body::Join(parts) => join(parts, k, env),
        Body::Div(a, b) => {
            join(std::slice::from_ref(a), k, env).and(join(std::slice::from_ref(b), k, env))
        }
        Body::Pow(base, _) => growth(&base.body, k, env),
        Body::Keyed { .. } => IndexGrowth::Polynomial,
        Body::Apply(Unary::Sin | Unary::Cos, arg) => bounded_along(arg, k, Axis::Real, env),
        Body::Apply(Unary::Exp, arg) if logarithmic(&arg.body, k) => IndexGrowth::Polynomial,
        Body::Apply(Unary::Exp, arg) => exponential_in(arg, k, env),
        Body::Channel(of, _) | Body::Crop { of, .. } => growth(&of.body, k, env),
        Body::Shift { of, .. } | Body::Deriv { of, .. } => growth(&of.body, k, env),
        Body::Warp { at, of } => match &*of.body {
            Body::Crop { of: inner, .. } => growth(&read_at(&inner.body, &at.body), k, env),
            other => growth(&read_at(other, &at.body), k, env),
        },
        Body::Pv(at) | Body::Delta { at, .. } => growth(&at.body, k, env),
        Body::Series(inner) => growth(&inner.term.body, k, env),
        _ => IndexGrowth::Unbounded,
    }
}

/// A turning exponent is bounded and a falling one is a geometric decay; only a rising real
/// part outgrows every polynomial.
fn exponential_in(arg: &Part, k: IndexId, env: &dyn Env) -> IndexGrowth {
    if let bounded @ IndexGrowth::Polynomial = bounded_along(arg, k, Axis::Imaginary, env) {
        return bounded;
    }
    match affine_in(&arg.body, Reading::Index(k)).and_then(|(slope, _)| slope.exact()) {
        Some(slope) if slope.re < 0.0 => IndexGrowth::Polynomial,
        _ => IndexGrowth::Unbounded,
    }
}

fn bounded_along(arg: &Part, k: IndexId, wanted: Axis, env: &dyn Env) -> IndexGrowth {
    if !mentions(&arg.body, k) || axis(&arg.body, env) == wanted {
        IndexGrowth::Polynomial
    } else {
        IndexGrowth::Unbounded
    }
}

/// An exponent reaching the index only through a logarithm is a power of it.
fn logarithmic(f: &Body, k: IndexId) -> bool {
    match f {
        _ if !mentions(f, k) => true,
        Body::Apply(Unary::Log, _) => true,
        Body::Add(parts) | Body::Mul(parts) => parts.iter().all(|p| logarithmic(&p.body, k)),
        Body::Div(a, b) => logarithmic(&a.body, k) && logarithmic(&b.body, k),
        _ => false,
    }
}

fn join(parts: &[Part], k: IndexId, env: &dyn Env) -> IndexGrowth {
    parts.iter().fold(IndexGrowth::Polynomial, |acc, p| {
        acc.and(growth(&p.body, k, env))
    })
}

impl IndexGrowth {
    fn and(self, other: IndexGrowth) -> IndexGrowth {
        match (self, other) {
            (IndexGrowth::Polynomial, IndexGrowth::Polynomial) => IndexGrowth::Polynomial,
            _ => IndexGrowth::Unbounded,
        }
    }
}

/// What separates a series term's coefficient from its wave.
pub fn mentions_line(f: &Body) -> bool {
    reaches(f, &|x| matches!(x, Body::Line))
}

/// A bound on `|c(k+1)| / |c(k)|` holding at every index.
pub fn ratio(f: &Body, k: IndexId) -> Option<f64> {
    if !mentions(f, k) {
        return Some(1.0);
    }
    match f {
        Body::Mul(parts) => parts
            .iter()
            .try_fold(1.0, |held, p| Some(held * ratio(&p.body, k)?)),
        Body::Add(parts) => parts.iter().try_fold(0.0f64, |held, p| match &*p.body {
            Body::Apply(Unary::Abs, _) => Some(held.max(ratio(&p.body, k)?)),
            _ => None,
        }),
        Body::Div(num, den) if !mentions(&den.body, k) => ratio(&num.body, k),
        Body::Apply(Unary::Abs, arg) => ratio(&arg.body, k),
        Body::Apply(Unary::Exp, arg) => {
            let (slope, _) = affine_in(&arg.body, Reading::Index(k))?;
            Some(slope.exact()?.re.exp())
        }
        Body::Pow(base, n) if *n >= 0 => Some(ratio(&base.body, k)?.powi(*n)),
        _ => None,
    }
}

pub fn mentions(f: &Body, k: IndexId) -> bool {
    reaches(f, &|x| matches!(x, Body::Index(i) if *i == k))
}

fn reaches(f: &Body, leaf: &dyn Fn(&Body) -> bool) -> bool {
    leaf(f) || children(f).iter().any(|p| reaches(&p.body, leaf))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub hz: f64,
    pub amp: C64,
}

/// `tail_db` is the loudest dropped line against the loudest taken one.
#[derive(Clone, Debug, PartialEq)]
pub struct Lines {
    pub taken: Vec<Line>,
    pub dropped: Vec<Line>,
    pub tail_db: f64,
}

pub const AUDIBLE_CEILING_HZ: f64 = 20_000.0;

/// Stops at the ceiling where the frequency closed form leaves the band. Where it never does, every
/// term piles onto one line: a geometric weight stops it where the whole tail it bounds is at
/// most `precision`, and any other where the loudest line falls under the floor.
pub fn lines(s: &Series, ceiling: f64, floor_db: f64, precision: f64) -> Lines {
    let ceiling = ceiling.min(AUDIBLE_CEILING_HZ);
    let Some(shape) = read(&s.term.body) else {
        return Lines {
            taken: Vec::new(),
            dropped: Vec::new(),
            tail_db: f64::NEG_INFINITY,
        };
    };
    let voices = places(&shape);
    let band = voices
        .iter()
        .filter_map(|(place, _)| leaves_band(place, s.index, ceiling))
        .fold(None, |held: Option<i64>, next| {
            Some(held.map_or(next, |held| held.max(next)))
        });
    let hi = match (s.hi, band) {
        (Bound::Finite(n), Some(last)) => n.min(last),
        (Bound::Finite(n), None) => n,
        (Bound::Infinite, Some(last)) => last,
        (Bound::Infinite, None) => s.lo.saturating_add(MAX_TERMS),
    };
    let ratio = voices
        .iter()
        .try_fold(0.0f64, |held, (_, weight)| {
            Some(held.max(ratio(weight, s.index)?))
        })
        .filter(|r| *r < 1.0);
    let floor = 10f64.powf(floor_db / 20.0);

    let mut taken = Vec::new();
    let mut dropped = Vec::new();
    let mut first = 0.0f64;
    for k in s.lo..=hi {
        let mut here = Vec::new();
        for (place, weight) in &voices {
            let (Some(hz), Some(amp)) = (
                at_index(place, s.index, k).map(|c| c.re),
                at_index(weight, s.index, k),
            ) else {
                continue;
            };
            here.push(Line { hz, amp });
        }
        let loudest = here.iter().map(|l| l.amp.abs()).fold(0.0f64, f64::max);
        if k == s.lo {
            first = loudest;
        }
        let bound: f64 = here.iter().map(|l| l.amp.abs()).sum();
        let gone = match ratio {
            Some(r) => here.len() == voices.len() && bound / (1.0 - r) <= precision,
            None => first > 0.0 && loudest < first * floor,
        };
        if band.is_none() && k > s.lo && gone {
            dropped.extend(here);
            break;
        }
        for line in here {
            if line.hz.abs() <= ceiling {
                taken.push(line);
            } else {
                dropped.push(line);
            }
        }
    }
    let loudest = |set: &[Line]| set.iter().map(|l| l.amp.abs()).fold(0.0f64, f64::max);
    let (kept, gone) = (loudest(&taken), loudest(&dropped));
    Lines {
        tail_db: if gone > 0.0 && kept > 0.0 {
            20.0 * (gone / kept).log10()
        } else {
            f64::NEG_INFINITY
        },
        taken,
        dropped,
    }
}

/// The step a series' own frequency walks: every term lands on a multiple of it. A
/// delta train's places are instants, not frequencies, and name no such step.
pub fn spacing(s: &Series) -> Option<f64> {
    let Some(shape @ Shape::Lines(_)) = read(&s.term.body) else {
        return None;
    };
    let mut held: Option<f64> = None;
    for (place, _) in places(&shape) {
        let (slope, offset) = affine_in(&place, Reading::Index(s.index))?;
        let (slope, offset) = (slope.exact()?.re, offset.exact()?.re);
        if slope == 0.0 || !slope.is_finite() || !offset.is_finite() {
            return None;
        }
        let steps = offset / slope;
        if (steps.round() - steps).abs() > TURN_EPSILON * steps.abs().max(1.0) {
            return None;
        }
        match held {
            Some(step) if step != slope.abs() => return None,
            _ => held = Some(slope.abs()),
        }
    }
    held
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enumerated {
    pub atoms: Vec<SpectralAtom>,
    pub dropped: Vec<Line>,
}

/// A crop of a series is the series of cropped terms: the window lifts off, goes back on
/// each. `None` where no line closed form reads under it. A delta's `hz` is an instant, not a pitch.
pub fn line_atoms(s: &Series, ceiling: f64, floor_db: f64, precision: f64) -> Option<Enumerated> {
    let (body, window) = crate::spectral_sum::image::crop_peeled(&s.term.body);
    let bare = Series {
        term: Part::new(s.term.origin, body),
        ..s.clone()
    };
    let singular = match read(&bare.term.body)? {
        Shape::Deltas(_) => true,
        Shape::Lines(_) => false,
    };
    let found = lines(&bare, ceiling, floor_db, precision);
    let atoms = found
        .taken
        .into_iter()
        .filter_map(|l| match singular {
            true => window.is_none_or(|w| w.contains(l.hz)).then(|| {
                SpectralAtom::new(
                    l.amp,
                    Factors::NONE,
                    Singular::Delta { at: l.hz, order: 0 },
                    s.term.origin,
                )
            }),
            false => Some(SpectralAtom::new(
                l.amp,
                Factors {
                    exp: Some(Exp::at(0.0, TAU * l.hz)),
                    ind: window,
                    ..Factors::NONE
                },
                Singular::Regular,
                s.term.origin,
            )),
        })
        .collect();
    Some(Enumerated {
        atoms,
        dropped: found.dropped,
    })
}

/// A whole turn count to floating precision: a tolerance would put a line on a neighbouring
/// bin, which `exact` cannot carry.
pub fn commensurate(hz: f64, horizon: f64) -> bool {
    let turns = hz * horizon;
    (turns.round() - turns).abs() <= TURN_EPSILON * turns.abs().max(1.0)
}

const TURN_EPSILON: f64 = 1e-9;

fn places(shape: &Shape) -> Vec<(Body, Body)> {
    match shape {
        Shape::Lines(lines) => lines
            .iter()
            .map(|l| (l.freq.clone(), l.amp.clone()))
            .collect(),
        Shape::Deltas(deltas) => deltas
            .iter()
            .map(|d| (d.at.clone(), d.weight.clone()))
            .collect(),
    }
}

/// The last index whose frequency still fits the band, solving `|slope*k + offset| <= ceiling`
/// at both signs: an offset opposing the slope carries the line back in before it leaves.
fn leaves_band(place: &Body, k: IndexId, ceiling: f64) -> Option<i64> {
    let (slope, offset) = affine_in(place, Reading::Index(k))?;
    let (slope, offset) = (slope.exact()?.re, offset.exact()?.re);
    if slope == 0.0 {
        return None;
    }
    let ends = [(ceiling - offset) / slope, (-ceiling - offset) / slope];
    let last = ends[0].max(ends[1]).floor();
    Some(last.clamp(0.0, MAX_TERMS as f64) as i64 + 1)
}

const MAX_TERMS: i64 = 1 << 20;

fn at_index(f: &Body, k: IndexId, value: i64) -> Option<C64> {
    exact_constant(&substitute(f, k, value as f64))
}

pub fn substitute(f: &Body, k: IndexId, value: f64) -> Body {
    match f {
        Body::Index(i) if *i == k => Body::Const(C64::real(value)),
        other => map_children(other, |p| {
            Part::new(p.origin, substitute(&p.body, k, value))
        }),
    }
}
