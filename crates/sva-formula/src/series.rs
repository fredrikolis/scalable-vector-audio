// Concern: decides a series' convergence in A and enumerates the lines it yields | Non-concern: placing them on a grid (sva-samples) | IO: (&Series, ceiling) -> bool, Lines, Enumerated

use std::f64::consts::TAU;

use crate::affine::{Axis, Reading, affine_in, affine_read, axis_read, exact_constant_at};
use crate::closed_form::{
    Body, Bound, IndexId, Part, Series, Unary, children, map_children, read_at,
};
use crate::complex::C64;
use crate::env::Env;
use crate::fourier_dual::series::{Shape, read_with};
use crate::spectral_sum::atom::{Exp, Factors, Singular, SpectralAtom};
use crate::through::{Opaque, Reads};

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

/// Decided from where the index sits, never by evaluating a term; a ref read at a time the
/// index moves is its form read there.
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
            Body::Node(id) => env
                .reads()
                .growth(*id, &at.body, k)
                .unwrap_or(IndexGrowth::Polynomial),
            other => growth(&read_at(other, &at.body), k, env),
        },
        Body::Pv(at) | Body::Delta { at, .. } => growth(&at.body, k, env),
        Body::Series(inner) => growth(&inner.term.body, k, env),
        _ => IndexGrowth::Unbounded,
    }
}

fn join(parts: &[Part], k: IndexId, env: &dyn Env) -> IndexGrowth {
    parts.iter().fold(IndexGrowth::Polynomial, |acc, p| {
        acc.and(growth(&p.body, k, env))
    })
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
    if !mentions(&arg.body, k) || axis_read(&arg.body, env, env.reads()) == wanted {
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
    mentions_line_read(f, &Opaque)
}

pub fn mentions_line_read(f: &Body, reads: &dyn Reads) -> bool {
    match f {
        Body::Line => true,
        Body::Node(id) => reads.mentions_line(*id),
        other => children(other)
            .iter()
            .any(|p| mentions_line_read(&p.body, reads)),
    }
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

/// Whether `|f(j)| <= |f(i)|` for every `j >= i >= 1`.
pub fn falls(f: &Body, k: IndexId) -> bool {
    power(f, k).is_some_and(|p| p >= 0.0)
}

/// Bounds every term from an index on by that term's magnitude alone.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Decay {
    Geometric(f64),
    Power(f64),
}

fn decay(f: &Body, k: IndexId) -> Option<Decay> {
    if let Some(r) = ratio(f, k).filter(|r| *r < 1.0) {
        return Some(Decay::Geometric(r));
    }
    power(f, k).filter(|p| *p > 1.0).map(Decay::Power)
}

impl Decay {
    /// `sum_{j>=k} |c(j)|` from `|c(k)|`; a power law by `sum_{j>k} (k/j)^p <= k/(p-1)`.
    fn tail(self, here: f64, k: i64) -> Option<f64> {
        match self {
            Decay::Geometric(r) => Some(here / (1.0 - r)),
            Decay::Power(p) => (k >= 1).then(|| here * (1.0 + k as f64 / (p - 1.0))),
        }
    }
}

/// A `p` with `|f(j)| <= |f(i)|*(i/j)^p` for every `j >= i >= 1`.
fn power(f: &Body, k: IndexId) -> Option<f64> {
    if let Some(e) = exponent(f, k) {
        return Some(-e);
    }
    match f {
        Body::Mul(parts) => parts
            .iter()
            .try_fold(0.0, |held, p| Some(held + power(&p.body, k)?)),
        Body::Add(parts) => parts
            .iter()
            .try_fold(f64::INFINITY, |held, p| match &*p.body {
                Body::Apply(Unary::Abs, _) => Some(held.min(power(&p.body, k)?)),
                _ => None,
            }),
        Body::Div(num, den) => Some(power(&num.body, k)? + rises(&den.body, k)?),
        Body::Apply(Unary::Abs, arg) => power(&arg.body, k),
        Body::Pow(base, n) if *n >= 0 => Some(power(&base.body, k)? * f64::from(*n)),
        _ => ratio(f, k).filter(|r| *r <= 1.0).map(|_| 0.0),
    }
}

/// An `e` with `|f(j)| >= |f(i)|*(j/i)^e` for every `j >= i >= 1`: `a*k + b`, `a > 0`,
/// `b <= 0 < a + b`, rises at least as `k` does.
fn rises(f: &Body, k: IndexId) -> Option<f64> {
    if let Some(e) = exponent(f, k) {
        return Some(e);
    }
    match f {
        Body::Pow(base, n) if *n >= 0 => Some(rises(&base.body, k)? * f64::from(*n)),
        _ => {
            let (slope, offset) = affine_in(f, Reading::Index(k))?;
            let (a, b) = (slope.exact()?, offset.exact()?);
            let real = a.im == 0.0 && b.im == 0.0;
            (real && a.re > 0.0 && b.re <= 0.0 && a.re + b.re > 0.0).then_some(1.0)
        }
    }
}

/// The `e` with `|f(j)| = |f(i)|*(j/i)^e` exactly for every `i, j >= 1`.
fn exponent(f: &Body, k: IndexId) -> Option<f64> {
    if !mentions(f, k) {
        return Some(0.0);
    }
    match f {
        Body::Index(i) if *i == k => Some(1.0),
        Body::Mul(parts) => parts
            .iter()
            .try_fold(0.0, |held, p| Some(held + exponent(&p.body, k)?)),
        Body::Div(num, den) => Some(exponent(&num.body, k)? - exponent(&den.body, k)?),
        Body::Apply(Unary::Abs, arg) => exponent(&arg.body, k),
        Body::Pow(base, n) => Some(exponent(&base.body, k)? * f64::from(*n)),
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
    /// Where the term's own frequency places it, which `hz` rounds.
    pub rung: Option<Rung>,
}

impl Line {
    pub fn bare(hz: f64, amp: C64) -> Line {
        Line {
            hz,
            amp,
            rung: None,
        }
    }
}

/// Exactly `offset + step*k` Hz: the frequency a series term writes at index `k`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rung {
    pub offset: f64,
    pub step: f64,
    pub k: i64,
}

/// `tail_db` is what was dropped against the loudest taken line: the loudest dropped line, or
/// where the walk stopped short, the bound on the summed magnitude of every term it left.
#[derive(Clone, Debug, PartialEq)]
pub struct Lines {
    pub taken: Vec<Line>,
    pub dropped: Vec<Line>,
    pub tail_db: f64,
}

/// Stops at the ceiling where the frequency closed form leaves the band. Where it never does, the
/// walk stops only where a decay bounds the whole tail: a geometric one at `precision`, a power
/// law under the floor against the loudest line taken. `None` where the term is no line, or an
/// infinite tail has no such bound.
pub fn lines(s: &Series, ceiling: f64, floor_db: f64, precision: f64) -> Option<Lines> {
    lines_read(s, (ceiling, floor_db, precision), &Opaque)
}

pub fn lines_read(s: &Series, band: (f64, f64, f64), reads: &dyn Reads) -> Option<Lines> {
    walk(s, band, reads)
}

fn walk(
    s: &Series,
    (ceiling, floor_db, precision): (f64, f64, f64),
    reads: &dyn Reads,
) -> Option<Lines> {
    let shape = read_with(&s.term.body, reads)?;
    let voices = places(&shape);
    let band = voices
        .iter()
        .filter_map(|(place, _)| leaves_band(place, s.index, ceiling, reads))
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
    let decays: Option<Vec<Decay>> = voices
        .iter()
        .map(|(_, weight)| decay(weight, s.index))
        .collect();
    let truncates = band.is_none() && s.hi == Bound::Infinite;
    let unread = |f: &Body| at_index(f, s.index, s.lo, reads).is_none();
    if truncates
        && voices
            .iter()
            .any(|(place, weight)| unread(place) || unread(weight))
    {
        return Some(Lines {
            taken: Vec::new(),
            dropped: Vec::new(),
            tail_db: f64::NEG_INFINITY,
        });
    }
    if truncates && ratio.is_none() && decays.is_none() {
        return None;
    }
    let floor = 10f64.powf(floor_db / 20.0);
    let ladders: Vec<Option<(f64, f64)>> = voices
        .iter()
        .map(|(place, _)| ladder(place, s.index, reads))
        .collect();

    let mut taken = Vec::new();
    let mut dropped = Vec::new();
    let mut peak = 0.0f64;
    let mut left = None;
    for k in s.lo..=hi {
        let mut here = Vec::new();
        for ((place, weight), ladder) in voices.iter().zip(&ladders) {
            let (Some(hz), Some(amp)) = (
                at_index(place, s.index, k, reads).map(|c| c.re),
                at_index(weight, s.index, k, reads),
            ) else {
                continue;
            };
            let rung = ladder.map(|(offset, step)| Rung { offset, step, k });
            here.push(Line { hz, amp, rung });
        }
        let whole = here.len() == voices.len();
        let tail = match (ratio, &decays) {
            (Some(r), _) => {
                whole.then(|| here.iter().map(|l| l.amp.abs()).sum::<f64>() / (1.0 - r))
            }
            (None, Some(decays)) if whole => here
                .iter()
                .zip(decays)
                .try_fold(0.0, |held, (l, d)| Some(held + d.tail(l.amp.abs(), k)?)),
            (None, _) => None,
        };
        let gone = tail.filter(|tail| match ratio {
            Some(_) => *tail <= precision,
            None => peak > 0.0 && *tail < peak * floor,
        });
        if truncates && k > s.lo && gone.is_some() {
            dropped.extend(here);
            left = gone;
            break;
        }
        for line in here {
            if line.hz.abs() < ceiling {
                peak = peak.max(line.amp.abs());
                taken.push(line);
            } else {
                dropped.push(line);
            }
        }
    }
    if truncates && left.is_none() {
        return None;
    }
    let loudest = |set: &[Line]| set.iter().map(|l| l.amp.abs()).fold(0.0f64, f64::max);
    let gone = loudest(&dropped).max(left.unwrap_or(0.0));
    Some(Lines {
        tail_db: if gone > 0.0 && peak > 0.0 {
            20.0 * (gone / peak).log10()
        } else {
            f64::NEG_INFINITY
        },
        taken,
        dropped,
    })
}

fn ladder(place: &Body, k: IndexId, reads: &dyn Reads) -> Option<(f64, f64)> {
    let (slope, offset) = affine_read(place, Reading::Index(k), reads)?;
    let (slope, offset) = (slope.exact()?, offset.exact()?);
    let real = slope.im == 0.0 && offset.im == 0.0;
    (real && slope.re.is_finite() && offset.re.is_finite()).then_some((offset.re, slope.re))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enumerated {
    pub atoms: Vec<SpectralAtom>,
    pub dropped: Vec<Line>,
}

/// A crop of a series is the series of cropped terms: the window lifts off, goes back on
/// each. `None` where no line closed form reads under it. A delta's `hz` is an instant, not a pitch.
pub fn line_atoms(s: &Series, ceiling: f64, floor_db: f64, precision: f64) -> Option<Enumerated> {
    line_atoms_read(s, (ceiling, floor_db, precision), &Opaque)
}

pub fn line_atoms_read(s: &Series, band: (f64, f64, f64), reads: &dyn Reads) -> Option<Enumerated> {
    let (body, window) =
        crate::spectral_sum::image::crop_peeled(&crate::through::looked(&s.term.body, reads));
    let bare = Series {
        term: Part::new(s.term.origin, body),
        ..s.clone()
    };
    let singular = match read_with(&bare.term.body, reads)? {
        Shape::Deltas(_) => true,
        Shape::Lines(_) => false,
    };
    let found = lines_read(&bare, band, reads)?;
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

/// The last index the walk reaches, solving `|slope*k + offset| <= ceiling`
/// at both signs: an offset opposing the slope carries the line back in before it leaves.
fn leaves_band(place: &Body, k: IndexId, ceiling: f64, reads: &dyn Reads) -> Option<i64> {
    let (slope, offset) = affine_read(place, Reading::Index(k), reads)?;
    let (slope, offset) = (slope.exact()?.re, offset.exact()?.re);
    if slope == 0.0 {
        return None;
    }
    let ends = [(ceiling - offset) / slope, (-ceiling - offset) / slope];
    let last = ends[0].max(ends[1]).floor();
    Some(last.clamp(0.0, MAX_TERMS as f64) as i64 + 1)
}

const MAX_TERMS: i64 = 1 << 20;

fn at_index(f: &Body, k: IndexId, value: i64, reads: &dyn Reads) -> Option<C64> {
    exact_constant_at(f, k, value as f64, reads)
}

pub fn substitute(f: &Body, k: IndexId, value: f64) -> Body {
    match f {
        Body::Index(i) if *i == k => Body::Const(C64::real(value)),
        other => map_children(other, |p| {
            Part::new(p.origin, substitute(&p.body, k, value))
        }),
    }
}

const MAX_WRITTEN_TERMS: i64 = 1 << 13;

/// `f`, each finite series summed term by term; `None` where one is infinite or too long.
pub fn written_out(f: &Body) -> Option<Body> {
    let mut left = MAX_WRITTEN_TERMS;
    within(f, &mut left)
}

/// `left` counts the terms every series written out so far may still add, nesting included.
fn within(f: &Body, left: &mut i64) -> Option<Body> {
    if let Body::Series(s) = f {
        let Bound::Finite(hi) = s.hi else {
            return None;
        };
        *left = left.checked_sub(hi.checked_sub(s.lo)?.checked_add(1)?.max(0))?;
        if *left < 0 {
            return None;
        }
        let terms = (s.lo..=hi)
            .map(|k| {
                Some(Part::new(
                    s.term.origin,
                    within(&substitute(&s.term.body, s.index, k as f64), left)?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        return Some(match terms.len() {
            0 => Body::Const(C64::ZERO),
            _ => Body::Add(terms),
        });
    }
    let mut whole = true;
    let out = map_children(f, |p| match within(&p.body, left) {
        Some(body) => Part::new(p.origin, body),
        None => {
            whole = false;
            p.clone()
        }
    });
    whole.then_some(out)
}
