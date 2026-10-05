// Concern: evaluates a spectral sum's atoms at one instant, and the crop and lane rules every evaluator shares | Non-concern: a written form's tree | IO: (&SpectralSum, t, component) -> C64

use sva_formula::closed_form::Unary;
use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};
use sva_formula::{C64, Lane, SpectralSum};

use crate::error::CollapseError;
use crate::grid::Grid;

/// Where a form is evaluated: a free instant, or a grid sample, whose edges the grid decides.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum At {
    Free(f64),
    Sample(Grid, i64),
}

impl At {
    pub fn t(self) -> f64 {
        match self {
            At::Free(t) => t,
            At::Sample(grid, n) => grid.instant(n),
        }
    }

    pub(crate) fn sample(self) -> Option<(Grid, i64)> {
        match self {
            At::Free(_) => None,
            At::Sample(grid, n) => Some((grid, n)),
        }
    }
}

type On = Option<(Grid, i64)>;

/// The six atom factors in closed form. A delta has no ordinary value, and a principal
/// value has none at its own pole.
pub(crate) fn eval_atom_on(a: &SpectralAtom, t: f64, on: On) -> Result<C64, CollapseError> {
    debug_assert!(
        on.is_none_or(|(grid, n)| grid.instant(n) == t),
        "a sample at its instant"
    );
    if let Singular::Delta { at, order } = a.sing {
        return Err(CollapseError::SingularInCt {
            at,
            order: i32::from(order),
        });
    }
    let held = match (a.ind, on) {
        (Some(ind), Some((grid, n))) => match grid.inside(n, ind.l.value(), ind.r.value()) {
            true => a.smooth_inside(t),
            false => Some(C64::ZERO),
        },
        _ => a.smooth_at(t),
    };
    held.ok_or(CollapseError::SingularInCt {
        at: a.pole.map_or(t, |p| p.at.re),
        order: -1,
    })
}

pub(crate) fn eval_lane_on(lane: &Lane, t: f64, on: On) -> Result<C64, CollapseError> {
    let mut sum = C64::ZERO;
    for a in &lane.atoms {
        sum = sum + eval_atom_on(a, t, on)?;
    }
    with_modal(lane, sum, t, on)
}

pub(crate) fn eval_among(
    lane: &Lane,
    live: &[usize],
    (grid, m): (Grid, i64),
) -> Result<C64, CollapseError> {
    let (t, on) = (grid.instant(m), Some((grid, m)));
    let mut sum = C64::ZERO;
    for &i in live {
        sum = sum + eval_atom_on(&lane.atoms[i], t, on)?;
    }
    with_modal(lane, sum, t, on)
}

fn with_modal(lane: &Lane, mut sum: C64, t: f64, on: On) -> Result<C64, CollapseError> {
    for bank in &lane.modal {
        for a in sva_formula::modal::atoms(bank, sva_formula::Origin::UNKNOWN) {
            sum = sum + eval_atom_on(&a, t, on)?;
        }
    }
    Ok(sum)
}

pub(crate) fn eval_spectral_sum(n: &SpectralSum, c: usize, t: f64) -> Result<C64, CollapseError> {
    eval_spectral_sum_on(n, c, At::Free(t))
}

pub(crate) fn eval_spectral_sum_on(
    n: &SpectralSum,
    c: usize,
    at: At,
) -> Result<C64, CollapseError> {
    eval_lane_on(&n.lanes[c.min(n.lanes.len() - 1)], at.t(), at.sample())
}

/// FORMAT 15.6's window: one over the plateau, a raised cosine over each shoulder, zero out.
pub fn crop_gain(t: f64, l: f64, r: f64, rise: f64, fall: f64) -> f64 {
    if t < l || t >= r {
        return 0.0;
    }
    shoulders(t, l, r, rise, fall)
}

/// The window's gain at an instant already known to lie inside it.
pub fn shoulders(t: f64, l: f64, r: f64, rise: f64, fall: f64) -> f64 {
    let opening = shoulder(t - l, rise);
    let closing = shoulder(r - t, fall);
    opening.min(closing)
}

fn shoulder(into: f64, span: f64) -> f64 {
    if span <= 0.0 || into >= span {
        return 1.0;
    }
    0.5 - 0.5 * (std::f64::consts::PI * into / span).cos()
}

/// Which operand of a `join` holds one component of the joined value, and which of its own
/// components that is.
pub fn lane_of(widths: &[usize], component: usize) -> Option<(usize, usize)> {
    let mut left = component;
    for (at, width) in widths.iter().enumerate() {
        if left < *width {
            return Some((at, left));
        }
        left -= width;
    }
    None
}

pub fn unary(op: Unary, x: C64) -> C64 {
    match op {
        Unary::Exp => x.exp(),
        Unary::Sin => C64::new(x.re.sin() * x.im.cosh(), x.re.cos() * x.im.sinh()),
        Unary::Cos => C64::new(x.re.cos() * x.im.cosh(), -x.re.sin() * x.im.sinh()),
        Unary::Tanh => C64::real(x.re.tanh()),
        Unary::Sat => C64::real(x.re.clamp(-1.0, 1.0)),
        Unary::Abs => C64::real(x.abs()),
        Unary::Log => C64::real(x.re.ln()),
        Unary::Sqrt => C64::real(x.re.sqrt()),
        Unary::Step => C64::real(sva_formula::affine::step(x.re)),
    }
}

/// How fast a carrier's angle turns at `t`, in radians a second.
pub(crate) fn turning(rate: &SpectralSum, t: f64) -> Result<f64, CollapseError> {
    Ok(eval_spectral_sum(rate, 0, t)?.re)
}
