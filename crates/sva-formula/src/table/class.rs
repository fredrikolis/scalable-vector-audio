// Concern: the abstract atom class and the image table's preconditions | Non-concern: transforming a concrete atom (the sibling families) | IO: (AtomClass) -> AtomClass or LeftReason

use crate::refusal::{Factor, LeftReason};

/// The rule table's preconditions read this alone, with no parameter values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Factors {
    pub poly: bool,
    pub exp: bool,
    pub gauss: bool,
    pub ind: bool,
    pub pole: bool,
    pub pv: bool,
    pub delta: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Growth {
    Tempered,
    GrowsLeft,
    GrowsRight,
    GrowsBoth,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtomClass {
    pub factors: Factors,
    pub growth: Growth,
    pub bounded: (bool, bool),
    pub pole_order: u16,
}

pub const MAX_POLE_ORDER: u16 = crate::rational::MAX_POLE_ORDER;

impl Growth {
    pub fn of_sigma(sigma: f64) -> Growth {
        match sigma {
            s if s > 0.0 => Growth::GrowsRight,
            s if s < 0.0 => Growth::GrowsLeft,
            _ => Growth::Tempered,
        }
    }

    pub fn join(self, other: Growth) -> Growth {
        match (self, other) {
            (Growth::Tempered, g) | (g, Growth::Tempered) => g,
            (a, b) if a == b => a,
            _ => Growth::GrowsBoth,
        }
    }

    fn settled(self, bounded: (bool, bool)) -> bool {
        let left = !matches!(self, Growth::GrowsLeft | Growth::GrowsBoth) || bounded.0;
        let right = !matches!(self, Growth::GrowsRight | Growth::GrowsBoth) || bounded.1;
        left && right
    }
}

impl AtomClass {
    pub fn regular() -> AtomClass {
        AtomClass {
            factors: Factors::default(),
            growth: Growth::Tempered,
            bounded: (false, false),
            pole_order: 0,
        }
    }
}

/// The pairs no row of the table covers, each beside the reason it names.
pub fn blocked_pair(f: Factors) -> Option<(Factor, Factor, LeftReason)> {
    match () {
        () if f.gauss && f.ind => Some((
            Factor::Gaussian,
            Factor::Indicator,
            LeftReason::GaussianTimesIndicator,
        )),
        () if f.gauss && f.pole => Some((
            Factor::Gaussian,
            Factor::Pole,
            LeftReason::GaussianTimesPole,
        )),
        () if f.pole && f.ind => Some((
            Factor::Pole,
            Factor::Indicator,
            LeftReason::PoleTimesIndicator,
        )),
        () => None,
    }
}

/// The order past which no row of the table names a residue.
pub fn over_pole_order(order: u16) -> Option<LeftReason> {
    (order > MAX_POLE_ORDER).then_some(LeftReason::PoleOrder(order))
}

/// The one source of truth for which classes have a dual in A, and which class it lands in.
pub fn dual_class(c: AtomClass) -> Result<AtomClass, LeftReason> {
    if let Some((_, _, reason)) = blocked_pair(c.factors) {
        return Err(reason);
    }
    if let Some(reason) = over_pole_order(c.pole_order) {
        return Err(reason);
    }
    if !c.growth.settled(c.bounded) {
        return Err(LeftReason::NotTempered);
    }
    Ok(image(c))
}

fn image(c: AtomClass) -> AtomClass {
    let f = c.factors;
    let out = match () {
        () if f.delta => Factors {
            poly: true,
            exp: true,
            ..Factors::default()
        },
        () if f.gauss => Factors {
            poly: f.poly,
            gauss: true,
            exp: true,
            ..Factors::default()
        },
        () if f.pv => Factors {
            ind: true,
            exp: true,
            ..Factors::default()
        },
        () if f.pole => Factors {
            poly: true,
            exp: true,
            ind: true,
            ..Factors::default()
        },
        () if f.ind || f.exp => Factors {
            pole: true,
            exp: true,
            pv: !f.exp && !bounded_both(c),
            delta: !f.exp && !bounded_both(c),
            ..Factors::default()
        },
        () => Factors {
            delta: true,
            ..Factors::default()
        },
    };
    AtomClass {
        factors: out,
        growth: Growth::Tempered,
        bounded: (out.ind, out.ind),
        pole_order: if out.pole { c.pole_order.max(1) } else { 0 },
    }
}

/// The Heaviside row: a one-sided bare indicator duals to a delta beside a principal value.
fn bounded_both(c: AtomClass) -> bool {
    c.bounded.0 && c.bounded.1
}
