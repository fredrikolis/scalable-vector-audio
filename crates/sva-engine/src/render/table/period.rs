// Concern: the whole samples after which a formula repeats exactly, where it does | Non-concern: folding a demand by it (segments.rs), evaluating it | IO: (form, its rows, grid) -> a period or none

use sva_formula::{Body, ClosedForm, Fold};
use sva_samples::{Grid, Rows};

use crate::time::{Lattice, Q};

/// Undamped commensurate lines repeat after `1/gcd`, a written `(a*t + b) % P` after `P/|a|`;
/// in samples, the numerator of `P*rate/step`.
pub(crate) fn period(form: Option<&ClosedForm>, rows: &Rows, grid: Grid) -> Option<i64> {
    let seconds = match rows.lines() {
        Some(hz) => lines(&hz)?,
        None => wrapped(&form?.body)?,
    };
    let samples = seconds
        .mul(Q::int(i64::from(grid.rate)))?
        .div(grid.step())?;
    i64::try_from(samples.num()).ok().filter(|n| *n > 0)
}

fn lines(hz: &[f64]) -> Option<Q> {
    let mut held: Option<Q> = None;
    for f in hz.iter().filter(|f| **f != 0.0) {
        let q = written(f.abs())?;
        held = Some(match held {
            None => q,
            Some(g) => gcd(g, q)?,
        });
    }
    Q::ONE.div(held?)
}

/// The decimal a product with 2π rounded away from.
fn written(hz: f64) -> Option<Q> {
    let ulp = f64::from_bits(hz.to_bits() + 1) - hz;
    (1..=15).find_map(|digits| {
        let text = format!("{:.*e}", digits - 1, hz);
        let near: f64 = text.parse().ok()?;
        ((near - hz).abs() <= 4.0 * ulp)
            .then(|| Q::decimal(near))
            .flatten()
    })
}

fn gcd(a: Q, b: Q) -> Option<Q> {
    let whole = |x: i128, y: i128| {
        let (mut x, mut y) = (x.abs(), y.abs());
        while y != 0 {
            (x, y) = (y, x % y);
        }
        x
    };
    let den = a.den().checked_mul(b.den())? / whole(a.den(), b.den());
    let (x, y) = (a.num() * (den / a.den()), b.num() * (den / b.den()));
    Q::new(whole(x, y), den)
}

fn wrapped(body: &Body) -> Option<Q> {
    let mut periods = Vec::new();
    if !mods(body, &mut periods) || periods.is_empty() {
        return None;
    }
    periods.into_iter().try_fold(None, |held: Option<Q>, p| {
        Some(Some(match held {
            None => p,
            Some(q) => q.mul(p)?.div(gcd(q, p)?)?,
        }))
    })?
}

fn mods(body: &Body, out: &mut Vec<Q>) -> bool {
    match body {
        Body::Line => false,
        Body::Fold(Fold::Mod, parts) => match parts.as_slice() {
            [x, p] => match (affine(&x.body), constant(&p.body)) {
                (Some(a), Some(p)) if !a.is_zero() && p > Q::ZERO => match p.div(abs(a)) {
                    Some(period) => {
                        out.push(period);
                        true
                    }
                    None => false,
                },
                _ => false,
            },
            _ => false,
        },
        other => sva_formula::closed_form::children(other)
            .iter()
            .all(|p| mods(&p.body, out)),
    }
}

fn abs(q: Q) -> Q {
    match q < Q::ZERO {
        true => q.neg(),
        false => q,
    }
}

fn affine(body: &Body) -> Option<Q> {
    let coeffs = sva_formula::affine::polynomial(body)?;
    let exact = |c: &sva_formula::affine::Coeff| {
        let c = c.exact()?;
        (c.im == 0.0).then(|| Q::decimal(c.re)).flatten()
    };
    match coeffs.as_slice() {
        [_, a] => exact(a),
        [_] => Some(Q::ZERO),
        _ => None,
    }
}

fn constant(body: &Body) -> Option<Q> {
    match body {
        Body::Const(c) if c.im == 0.0 => Q::decimal(c.re),
        _ => None,
    }
}
