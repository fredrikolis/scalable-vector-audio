// Concern: multiplies a series by a finite atom sum, moving or weighting each line | Non-concern: multiplying two atoms (product.rs) | IO: (&Series, &[SpectralAtom]) -> Vec<Series> or Left

use std::f64::consts::TAU;

use crate::closed_form::{Body, Part, Series, Unary};
use crate::complex::C64;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Indicator, Singular, SpectralAtom};
use crate::table::series::{Shape, SymDelta, SymLine, read, write};

/// A finite line sum times a line series is one series per factor, each moved by that
/// factor's own frequency and cropped by its own window. A closed form in `f` folds instead.
pub fn times_atoms(s: &Series, by: &[SpectralAtom]) -> Result<Vec<Series>, Left> {
    let origin = by.first().map_or(s.term.origin, |a| a.origin);
    let refuse = || {
        Left::new(
            origin,
            AtomSketch::of(Factor::Value),
            LeftReason::SeriesNonUniform,
        )
    };
    let (body, held) = crate::spectral_sum::image::crop_peeled(&s.term.body);
    match read(&body) {
        Some(Shape::Deltas(_)) if held.is_none() => Ok(vec![times_lines(s, by)?]),
        Some(Shape::Lines(lines)) => by.iter().map(|a| moved(s, &lines, a, held)).collect(),
        _ => Err(refuse()),
    }
}

/// A bare turning exponential is a gain and a frequency offset; nothing else is one value
/// across a whole series.
fn moved(
    s: &Series,
    lines: &[SymLine],
    by: &SpectralAtom,
    held: Option<Indicator>,
) -> Result<Series, Left> {
    if by.poly > 0
        || by.gauss.is_some()
        || by.pole.is_some()
        || !matches!(by.sing, Singular::Regular)
        || by.exp.is_some_and(|e| e.sigma != 0.0)
    {
        return Err(Left::new(
            by.origin,
            AtomSketch::of(Factor::Value),
            LeftReason::SeriesNonUniform,
        ));
    }
    let offset = by.exp.map_or(0.0, |e| e.omega / TAU);
    let moved: Vec<SymLine> = lines
        .iter()
        .map(|l| SymLine {
            amp: product(l.amp.clone(), Body::Const(by.c)),
            freq: Body::Add(vec![
                Part::bare(l.freq.clone()),
                Part::bare(Body::Const(C64::real(offset))),
            ]),
        })
        .collect();
    let term = write(&Shape::Lines(moved));
    let window = match (held, by.ind) {
        (Some(held), Some(own)) => Some(held.meet(own)),
        (held, own) => held.or(own),
    };
    Ok(Series {
        term: Part::new(
            s.term.origin,
            match window {
                Some(window) => Body::Crop {
                    of: Part::new(s.term.origin, term),
                    l: window.l,
                    r: window.r,
                    rise: 0.0,
                    fall: 0.0,
                },
                None => term,
            },
        ),
        ..s.clone()
    })
}

/// A factor multiplied into a delta series folds to the factor read at each line's own
/// frequency, which is FORMAT 10.1's `H(f)*delta(f - f0)` once per term of the series.
fn times_lines(s: &Series, by: &[SpectralAtom]) -> Result<Series, Left> {
    let origin = by.first().map_or(s.term.origin, |a| a.origin);
    let refuse = || {
        Left::new(
            origin,
            AtomSketch::of(Factor::Value),
            LeftReason::SeriesNonUniform,
        )
    };
    let Some(Shape::Deltas(deltas)) = read(&s.term.body) else {
        return Err(refuse());
    };
    let mut shaped = Vec::with_capacity(deltas.len());
    for d in deltas {
        let gain = read_at(by, &d.at).ok_or_else(refuse)?;
        shaped.push(SymDelta {
            weight: product(d.weight, gain),
            at: d.at,
        });
    }
    Ok(Series {
        term: Part::new(s.term.origin, write(&Shape::Deltas(shaped))),
        ..s.clone()
    })
}

fn read_at(by: &[SpectralAtom], at: &Body) -> Option<Body> {
    let mut sum: Option<Body> = None;
    for a in by {
        let term = atom_at(a, at)?;
        sum = Some(match sum {
            None => term,
            Some(held) => Body::Add(vec![Part::bare(held), Part::bare(term)]),
        });
    }
    sum
}

/// A Gaussian, an indicator and a delta name no value at one line.
fn atom_at(a: &SpectralAtom, at: &Body) -> Option<Body> {
    if a.gauss.is_some() || a.ind.is_some() || a.is_delta() {
        return None;
    }
    let mut out = Body::Const(a.c);
    if a.poly > 0 {
        out = product(out, Body::Pow(Part::bare(at.clone()), i32::from(a.poly)));
    }
    if let Some(e) = a.exp {
        let rate = Body::Const(C64::new(e.sigma, e.omega));
        out = product(
            out,
            Body::Apply(Unary::Exp, Part::bare(product(rate, at.clone()))),
        );
    }
    if let Some(p) = a.pole {
        if p.pv {
            return None;
        }
        let offset = Body::Add(vec![Part::bare(at.clone()), Part::bare(Body::Const(-p.at))]);
        out = Body::Div(
            Part::bare(out),
            Part::bare(Body::Pow(Part::bare(offset), i32::from(p.order))),
        );
    }
    Some(out)
}

fn product(a: Body, b: Body) -> Body {
    Body::Mul(vec![Part::bare(a), Part::bare(b)])
}
