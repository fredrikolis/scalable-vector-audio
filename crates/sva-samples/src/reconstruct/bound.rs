// Concern: the proven error of a kernel reading, per source component below and above the band edge | Non-concern: computing a reading (reconstruct.rs) | IO: (kernel, band) -> Bound

use std::f64::consts::PI;

use super::table;
use super::tails::Tails;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bound {
    pub kernel: &'static str,
    pub taps: usize,
    pub band_hz: f64,
    pub in_band: f64,
    pub above_band: f64,
    pub lebesgue: f64,
    pub position: f64,
}

/// Rounding, to 512 taps.
pub(super) const ROUNDING: f64 = 1e-12;

pub(super) fn of(
    kernel: &'static str,
    (half_width, oversample): (usize, usize),
    (beta, lobe): (f64, f64),
    nu: f64,
    lebesgue: f64,
) -> Option<Bound> {
    let tails = Tails::new(beta, half_width as f64, lobe);
    if 0.5 - nu < lobe {
        return None;
    }
    let table = table::error(half_width, oversample, beta);
    Some(Bound {
        kernel,
        taps: 2 * half_width,
        band_hz: nu,
        in_band: tails.in_band(nu) + table + ROUNDING,
        above_band: tails.above_band(nu) + table + ROUNDING,
        lebesgue,
        position: 0.0,
    })
}

impl Bound {
    /// `2 pi nu delta` per component at `nu` cycles a sample.
    pub fn moved(self, delta: f64, source_rate: u32) -> Bound {
        let nu = self.band_hz / f64::from(source_rate);
        Bound {
            in_band: self.in_band + 2.0 * PI * nu * delta,
            above_band: self.above_band + PI * delta,
            position: self.position + delta,
            ..self
        }
    }
}
