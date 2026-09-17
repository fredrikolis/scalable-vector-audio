// Concern: whether a dual fits the band, and a Gaussian's reach past it | Non-concern: what a crop truncates (truncate.rs), evaluating one (point.rs) | IO: (a sum or an atom, ceiling) -> bool, dB

use std::f64::consts::PI;

use sva_formula::SpectralSum;
use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};

use crate::profile::Profile;

/// A Gaussian duals to `sqrt(pi/a) exp(-pi^2 f^2/a)`: this is that edge, in dB.
pub fn edge_db(a: &SpectralAtom, ceiling: f64) -> f64 {
    a.gauss.map_or(0.0, |g| {
        -20.0 * PI * PI * ceiling * ceiling / g.a / std::f64::consts::LN_10
    })
}

pub fn band_limited(n: &SpectralSum, ceiling: f64, profile: &Profile) -> bool {
    n.lanes.iter().all(|lane| {
        lane.series.is_empty()
            && !lane.atoms.is_empty()
            && lane.atoms.iter().all(|a| {
                a.ind.is_none()
                    && a.pole.is_none()
                    && matches!(a.sing, Singular::Regular)
                    && a.gauss.is_some()
                    && edge_db(a, ceiling) < profile.floor(ceiling)
            })
    })
}

pub fn windowed(n: &SpectralSum) -> bool {
    n.atoms().any(|a| a.ind.is_some())
}
