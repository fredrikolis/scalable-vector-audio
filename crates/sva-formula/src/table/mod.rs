// Concern: dispatches one atom to its family and versions the whole table | Non-concern: any family's own image (the sibling files) | IO: (&SpectralSum) -> SpectralSum or Left

pub(crate) mod class;
pub mod exponential;
pub mod gaussian;
pub mod indicator;
pub mod pole;
pub mod series;

use crate::closed_form::{Edge, Var};
use crate::modal;
use crate::refusal::{AtomSketch, Factor, Left, LeftReason};
use crate::spectral_sum::atom::{Exp, Factors, Gauss, Indicator, Pole, Singular, SpectralAtom};
use crate::spectral_sum::merge::simplify;
use crate::spectral_sum::{Lane, SpectralSum};
use class::{blocked_pair, over_pole_order};

/// Every hash feeds this first, so a bump retires each entry written before it. 3: a crop
/// hashes its shoulders, and the Gaussian row centres its image.
pub const TABLE_VERSION: u64 = 3;

/// The families this table duals.
pub const FAMILIES: [(&str, &str); 5] = [
    (
        "exponential",
        "c t^n e^{(sigma + i omega) t} and delta^(k), duals to the same shape in u",
    ),
    (
        "gaussian",
        "e^{-a (t - mu)^2}, duals to a Gaussian by completing the square",
    ),
    (
        "indicator",
        "1[l,r](u), duals to pole atoms and a principal value",
    ),
    (
        "pole",
        "c/(u - p)^m and pv, duals to one-sided exponentials and sgn",
    ),
    (
        "series",
        "a sum, dualed termwise between a line and a delta",
    ),
];

/// One kernel, `integral x(u) e^{-2*pi*i*theta*u} du`, in whichever variable the form
/// holds. `dual(dual(x)) = x(-u)`, so `ifourier` is this then `reflect`.
pub fn dual(n: &SpectralSum) -> Result<SpectralSum, Left> {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in &n.lanes {
        let mut atoms = Vec::new();
        for a in expanded(lane)? {
            atoms.extend(dual_atom(&a)?);
        }
        let mut out = Lane {
            atoms,
            series: lane
                .series
                .iter()
                .map(series::dual)
                .collect::<Result<_, _>>()?,
            modal: Vec::new(),
        };
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(SpectralSum::of(other(n.var), lanes))
}

/// `x(-u)`: the reflection that turns a forward transform into an inverse one.
pub fn reflect(n: &SpectralSum) -> Result<SpectralSum, Left> {
    let mut lanes = Vec::with_capacity(n.lanes.len());
    for lane in &n.lanes {
        let mut out = Lane {
            atoms: expanded(lane)?.iter().map(reflect_atom).collect(),
            series: lane
                .series
                .iter()
                .map(series::reflect)
                .collect::<Result<_, _>>()?,
            modal: Vec::new(),
        };
        simplify(&mut out);
        lanes.push(out);
    }
    Ok(SpectralSum::of(n.var, lanes))
}

/// A transform reads the exponential from the origin, so a shifted reference moves back.
fn flattened(a: &SpectralAtom) -> Result<SpectralAtom, Left> {
    let Some(e) = a.exp else {
        return Ok(*a);
    };
    let Some((held, carried)) = e.flattened() else {
        return Err(Left::new(
            a.origin,
            AtomSketch::of(Factor::Exponential),
            LeftReason::NotTempered,
        ));
    };
    Ok(a.with(
        a.c * carried,
        Factors {
            exp: Some(held),
            ..a.factors()
        },
    ))
}

fn other(var: Var) -> Var {
    match var {
        Var::T => Var::F,
        Var::F => Var::T,
    }
}

/// A modal bank is finitely many modes, so both directions read it as the atoms it is.
fn expanded(lane: &Lane) -> Result<Vec<SpectralAtom>, Left> {
    let mut atoms = lane.atoms.clone();
    for bank in &lane.modal {
        let origin = atoms
            .first()
            .map_or(crate::origin::Origin::UNKNOWN, |a| a.origin);
        atoms.extend(modal::atoms(bank, origin));
    }
    Ok(atoms)
}

fn factors_of(a: &SpectralAtom) -> class::Factors {
    class::Factors {
        poly: a.poly > 0,
        exp: a.exp.is_some(),
        gauss: a.gauss.is_some(),
        ind: a.ind.is_some(),
        pole: a.pole.is_some(),
        pv: a.pole.is_some_and(|p| p.pv),
        delta: a.is_delta(),
    }
}

pub fn dual_atom(a: &SpectralAtom) -> Result<Vec<SpectralAtom>, Left> {
    let a = &flattened(a)?;
    if let Some(reason) = over_pole_order(a.pole.map_or(0, |p| p.order)) {
        return Err(Left::new(a.origin, AtomSketch::of(Factor::Pole), reason));
    }
    if a.is_delta() {
        return Ok(exponential::delta_row(a));
    }
    if let Some((first, second, reason)) = blocked_pair(factors_of(a)) {
        return Err(Left::new(a.origin, AtomSketch::pair(first, second), reason));
    }
    if a.gauss.is_some() {
        return Ok(gaussian::row(a));
    }
    if a.pole.is_some() {
        return pole::row(a);
    }
    if a.ind.is_some() {
        return indicator::row(a);
    }
    if a.exp.is_some_and(|e| e.sigma != 0.0) {
        return Err(Left::new(
            a.origin,
            AtomSketch::of(Factor::Exponential),
            LeftReason::NotTempered,
        ));
    }
    Ok(exponential::line_row(a))
}

fn reflect_atom(a: &SpectralAtom) -> SpectralAtom {
    if let Singular::Delta { at, order } = a.sing {
        return SpectralAtom::new(
            a.c.scale(if order % 2 == 0 { 1.0 } else { -1.0 }),
            Factors::NONE,
            Singular::Delta { at: -at, order },
            a.origin,
        );
    }
    let f = a.factors();
    let poles = f.pole.map_or(0, |p| p.order);
    let sign = if (usize::from(a.poly) + usize::from(poles)) % 2 == 0 {
        1.0
    } else {
        -1.0
    };
    SpectralAtom::new(
        a.c.scale(sign),
        Factors {
            poly: a.poly,
            exp: f.exp.map(|e| Exp {
                sigma: -e.sigma,
                omega: -e.omega,
                mu: -e.mu,
            }),
            gauss: f.gauss.map(|g| Gauss { mu: -g.mu, ..g }),
            ind: f.ind.map(|i| Indicator {
                l: Edge::at(-i.r.value()),
                r: Edge::at(-i.l.value()),
            }),
            pole: f.pole.map(|p| Pole { at: -p.at, ..p }),
        },
        Singular::Regular,
        a.origin,
    )
}

/// The dual of a whole closed form in `t` read back in `t`: what `ifourier` answers.
pub fn inverse(n: &SpectralSum) -> Result<SpectralSum, Left> {
    reflect(&dual(n)?)
}
