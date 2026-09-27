// Concern: a level a signal provably returns to forever, through maps and sums | Non-concern: the upper bound beside it | IO: (atoms, rate) or (floors, tails) -> a level

use std::collections::BTreeMap;

use sva_formula::spectral_sum::atom::SpectralAtom;
use sva_formula::spectral_sum::sup::{Fate, fate};
use sva_formula::{C64, Unary};

/// Distinct steady lines under Nyquist return to their root-sum-square forever.
pub(super) fn of_atoms(atoms: &[SpectralAtom], rate: f64) -> f64 {
    let mut lines: BTreeMap<u64, C64> = BTreeMap::new();
    let mut grows = false;
    for atom in atoms {
        let hz = atom.exp.map_or(0.0, |e| e.omega) / std::f64::consts::TAU;
        let held = fate(atom);
        if held != Fate::Decays && hz.abs() >= rate / 2.0 {
            return 0.0;
        }
        match held {
            Fate::Grows => grows = true,
            Fate::Steady => {
                let line = lines.entry(hz.to_bits()).or_insert(C64::ZERO);
                *line = *line + atom.c;
            }
            Fate::Decays => {}
        }
    }
    match grows {
        true => f64::INFINITY,
        false => lines.values().map(|c| c.norm_sqr()).sum::<f64>().sqrt(),
    }
}

pub(super) fn through(op: Unary, floor: f64) -> f64 {
    match op {
        Unary::Abs => floor,
        Unary::Tanh => floor.tanh(),
        Unary::Sat => floor.min(1.0),
        Unary::Sqrt => floor.sqrt(),
        Unary::Sin | Unary::Cos | Unary::Exp | Unary::Log => 0.0,
    }
}

pub(super) fn bounded_away(name: &str, constants: &[Option<f64>]) -> f64 {
    let pick = |keep: fn(&f64) -> bool| {
        constants
            .iter()
            .flatten()
            .filter(|c| keep(c))
            .fold(0.0f64, |held, c| held.max(c.abs()))
    };
    match name {
        "max" => pick(|c| *c > 0.0),
        "min" => pick(|c| *c < 0.0),
        _ => 0.0,
    }
}

/// `limsup |sum x_k| >= floor_i - sum_{k != i} tail_k`, the best over `i`.
pub(super) fn summed(floors: &[f64], tails: &[f64]) -> f64 {
    let total: f64 = tails.iter().sum();
    floors
        .iter()
        .zip(tails)
        .map(|(floor, tail)| floor - (total - tail))
        .fold(0.0f64, f64::max)
}
