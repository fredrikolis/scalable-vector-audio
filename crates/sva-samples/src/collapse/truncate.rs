// Concern: truncates every series to the terms the profile leaves, once per collapse | Non-concern: evaluating what is left (point.rs) | IO: (&SpectralSum or &Body) -> the same, series-free

use std::f64::consts::TAU;

use sva_formula::affine::exact_constant;
use sva_formula::closed_form::{Bound, Series, children, map_children};
use sva_formula::series::{mentions_line, substitute};
use sva_formula::spectral_sum::atom::{Exp, Factors, Singular, SpectralAtom};
use sva_formula::table::series::{Shape, read};
use sva_formula::{Body, C64, Lane, Part, SpectralSum, Var, lines};

use crate::error::CollapseError;
use crate::profile::Profile;

/// What a series is truncated against: the observation's own ceiling and audibility floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Audible {
    ceiling: f64,
    floor_db: f64,
}

impl Audible {
    pub fn of(profile: &Profile, rate: u32) -> Audible {
        let ceiling = profile.ceiling(rate);
        Audible {
            ceiling,
            floor_db: profile.floor(ceiling),
        }
    }
}

/// A line series is placed analytically under `sva_formula`'s own bound; an expanded one
/// becomes a formula the sample loop walks, which is what this caps.
const MAX_EXPANDED_TERMS: usize = 1 << 13;

/// FORMAT 6.2 truncates a series once, at collapse: every term the ceiling and the floor
/// leave becomes an ordinary atom before the first sample is read.
pub fn spectral_sum(n: &SpectralSum, band: Audible) -> Result<SpectralSum, CollapseError> {
    if n.lanes.iter().all(|l| l.series.is_empty()) {
        return Ok(n.clone());
    }
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in &n.lanes {
        let mut held = Lane {
            series: Vec::new(),
            ..lane.clone()
        };
        for s in &lane.series {
            held.atoms.extend(atoms(s, band)?);
        }
        lanes.push(held);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

/// The same truncation over a written closed form, whose series a spectral sum never reached.
pub fn written(f: &Body, band: Audible) -> Result<Body, CollapseError> {
    if let Body::Series(s) = f {
        return expanded(s, band);
    }
    let mut found = None;
    let out = map_children(f, |p| match written(&p.body, band) {
        Ok(body) => Part::new(p.origin, body),
        Err(e) => {
            found = Some(e);
            p.clone()
        }
    });
    match found {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

fn atoms(s: &Series, band: Audible) -> Result<Vec<SpectralAtom>, CollapseError> {
    let (body, window) = sva_formula::crop_peeled(&s.term.body);
    let bare = Series {
        term: Part::new(s.term.origin, body),
        ..s.clone()
    };
    let taken = enumerated(&bare, band);
    if !taken.is_empty() {
        return Ok(taken
            .into_iter()
            .map(|l| {
                SpectralAtom::new(
                    l.amp,
                    Factors {
                        exp: Some(Exp::at(0.0, TAU * l.hz)),
                        ind: window,
                        ..Factors::NONE
                    },
                    Singular::Regular,
                    s.term.origin,
                )
            })
            .collect());
    }
    let body = expanded(s, band)?;
    let held = sva_formula::normalize(&body, Var::T)
        .map_err(|_| CollapseError::NotEvaluable("a series term"))?;
    Ok(held.lanes.into_iter().flat_map(|l| l.atoms).collect())
}

/// A line series places its terms analytically; anything else is summed term by term.
fn expanded(s: &Series, band: Audible) -> Result<Body, CollapseError> {
    let taken = enumerated(s, band);
    if !taken.is_empty() {
        return Ok(sum(taken.iter().map(wave).collect()));
    }
    let count = terms(s, band)?;
    let mut parts = Vec::with_capacity(count);
    for i in 0..count {
        let form = substitute(&s.term.body, s.index, (s.lo + i as i64) as f64);
        parts.push(written(&form, band)?);
    }
    Ok(sum(parts))
}

fn enumerated(s: &Series, band: Audible) -> Vec<sva_formula::Line> {
    match read(&s.term.body) {
        Some(Shape::Lines(_)) => lines(s, band.ceiling, band.floor_db).taken,
        _ => Vec::new(),
    }
}

fn wave(l: &sva_formula::Line) -> Body {
    Body::Mul(vec![
        Part::bare(Body::Const(l.amp)),
        Part::bare(Body::Apply(
            sva_formula::Unary::Exp,
            Part::bare(Body::Mul(vec![
                Part::bare(Body::Const(C64::new(0.0, TAU * l.hz))),
                Part::bare(Body::Line),
            ])),
        )),
    ])
}

fn sum(parts: Vec<Body>) -> Body {
    match parts.len() {
        0 => Body::Const(C64::ZERO),
        1 => parts.into_iter().next().expect("one part"),
        _ => Body::Add(parts.into_iter().map(Part::bare).collect()),
    }
}

/// A nesting is priced whole before it expands: a written bound counts like the floor's.
fn terms(s: &Series, band: Audible) -> Result<usize, CollapseError> {
    let cost = cost(s, band).ok_or(CollapseError::NotEvaluable(
        "a series whose term count no coefficient bounds",
    ))?;
    if cost.depth == 1 && cost.own > MAX_EXPANDED_TERMS {
        return Err(CollapseError::NotEvaluable(
            "a series of more terms than one instant expands",
        ));
    }
    match cost.terms > MAX_EXPANDED_TERMS {
        true => Err(CollapseError::NestedSeries {
            depth: cost.depth,
            terms: cost.terms,
            bound: MAX_EXPANDED_TERMS,
        }),
        false => Ok(cost.own),
    }
}

/// Series deep, terms the whole nesting takes, terms this level alone takes.
struct Cost {
    depth: usize,
    terms: usize,
    own: usize,
}

fn cost(s: &Series, band: Audible) -> Option<Cost> {
    let own = counted(s, band)?;
    let inside = substitute(&s.term.body, s.index, s.lo as f64);
    let (depth, inner) = price(&inside, band)?;
    Some(Cost {
        depth: depth + 1,
        terms: own.saturating_mul(inner),
        own,
    })
}

/// A sum of series costs their counts added; every other spelling multiplies.
fn price(f: &Body, band: Audible) -> Option<(usize, usize)> {
    if let Body::Series(s) = f {
        let cost = cost(s, band)?;
        return Some((cost.depth, cost.terms));
    }
    let summed = matches!(f, Body::Add(_));
    let mut depth = 0;
    let mut terms: usize = if summed { 0 } else { 1 };
    for p in children(f) {
        let (d, n) = price(&p.body, band)?;
        depth = depth.max(d);
        terms = match summed {
            true => terms.saturating_add(n),
            false => terms.saturating_mul(n),
        };
    }
    Some((depth, terms.max(1)))
}

fn counted(s: &Series, band: Audible) -> Option<usize> {
    let hi = match s.hi {
        Bound::Finite(n) => return usize::try_from((n - s.lo + 1).max(0)).ok(),
        Bound::Infinite => MAX_EXPANDED_TERMS,
    };
    let coefficient = coefficient(&s.term.body)?;
    let floor = 10f64.powf(band.floor_db / 20.0);
    let mut peak = 0.0f64;
    for i in 0..hi {
        let at = substitute(&coefficient, s.index, (s.lo + i as i64) as f64);
        let held = exact_constant(&at)?.abs();
        peak = peak.max(held);
        if peak > 0.0 && held < peak * floor {
            return Some(i.max(1));
        }
    }
    None
}

/// A turning factor stands for the bound it turns inside; one with no bound leaves the
/// count undecided. A node, a series and a warp turn too: the index reaches each only as a
/// shift.
fn coefficient(f: &Body) -> Option<Body> {
    let one = || Some(Body::Const(C64::ONE));
    match f {
        Body::Line | Body::Node(_) | Body::Series(_) => one(),
        Body::Mul(parts) | Body::Add(parts) => {
            let mut kept = Vec::with_capacity(parts.len());
            for p in parts {
                kept.push(Part::new(p.origin, coefficient(&p.body)?));
            }
            Some(match f {
                Body::Mul(_) => Body::Mul(kept),
                _ => Body::Add(kept),
            })
        }
        Body::Div(a, b) => Some(Body::Div(
            Part::new(a.origin, coefficient(&a.body)?),
            Part::new(b.origin, coefficient(&b.body)?),
        )),
        Body::Apply(
            sva_formula::Unary::Sin
            | sva_formula::Unary::Cos
            | sva_formula::Unary::Tanh
            | sva_formula::Unary::Sat,
            _,
        ) => one(),
        Body::Crop { of, .. } | Body::Shift { of, .. } | Body::Warp { of, .. } => {
            coefficient(&of.body)
        }
        Body::Pow(base, n) => Some(Body::Pow(
            Part::new(base.origin, coefficient(&base.body)?),
            *n,
        )),
        other if !mentions_line(other) => Some(other.clone()),
        _ => None,
    }
}
