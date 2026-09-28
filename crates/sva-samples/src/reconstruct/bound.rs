// Concern: the proven error of a kernel reading, per source component below and above the band edge | Non-concern: computing a reading (reconstruct.rs) | IO: (spec, beta, band) -> Bound

use super::KernelSpec;
use super::table;
use super::tails::Tails;

/// A source component below `band_hz` is read within `in_band` of its amplitude, one above
/// within `above_band`, an error in the samples within `lebesgue` times it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bound {
    pub kernel: &'static str,
    pub taps: usize,
    pub band_hz: f64,
    pub in_band: f64,
    pub above_band: f64,
    pub lebesgue: f64,
}

/// A table entry's and a dot product's rounding, at most 2N taps each under one.
pub(super) const ROUNDING: f64 = 1e-12;

pub(super) fn of(spec: KernelSpec, beta: f64, nu: f64, lebesgue: f64) -> Option<Bound> {
    let tails = Tails::new(beta, spec.half_width as f64);
    if 0.5 - nu < tails.lobe {
        return None;
    }
    let table = table::error(spec, beta);
    Some(Bound {
        kernel: spec.name,
        taps: 2 * spec.half_width,
        band_hz: 0.0,
        in_band: tails.in_band(nu) + table + ROUNDING,
        above_band: tails.above_band(nu) + table + ROUNDING,
        lebesgue,
    })
}
