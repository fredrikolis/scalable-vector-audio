// Concern: a rectangular membrane's eigenfrequencies | Non-concern: a struck membrane's contact force (mod.rs) | IO: (MembraneGeom, count) -> Vec<Mode>

use crate::closed_form::Mode;
use crate::modal::damping::Damping;
use crate::modal::grid::{lowest, reach};
use crate::modal::mode;

pub struct MembraneGeom {
    pub lx: f64,
    pub ly: f64,
    pub tension: f64,
    pub density: f64,
    pub strike: (f64, f64),
    pub damping: Damping,
}

/// Morse & Ingard, Theoretical Acoustics, sec. 5.2; the amplitude is the mode shape.
pub fn modes(g: &MembraneGeom, count: usize) -> Vec<Mode> {
    let speed = (g.tension / g.density).sqrt();
    let span = reach(&[g.lx, g.ly], count);
    let mut found = Vec::new();
    for m in 1..=span[0] {
        for n in 1..=span[1] {
            let (mf, nf) = (m as f64, n as f64);
            let hz = 0.5 * speed * ((mf / g.lx).powi(2) + (nf / g.ly).powi(2)).sqrt();
            let shape = (mf * std::f64::consts::PI * g.strike.0).sin()
                * (nf * std::f64::consts::PI * g.strike.1).sin();
            found.push(mode(hz, shape, g.damping));
        }
    }
    lowest(found, count)
}
