// Concern: a free-free bar's eigenfrequencies | Non-concern: a clamped bar's boundary | IO: (BarGeom, count) -> Vec<Mode>

use crate::closed_form::Mode;
use crate::modal::damping::Damping;
use crate::modal::mode;

pub struct BarGeom {
    pub length: f64,
    pub thickness: f64,
    pub young: f64,
    pub density: f64,
    pub damping: Damping,
}

/// Roots of `cos(beta)*cosh(beta) = 1`; Fletcher & Rossing 2e, Table 2.2.
pub const BETA: [f64; 3] = [4.730_040_74, 7.853_204_62, 10.995_607_84];

pub fn beta(ordinal: usize) -> f64 {
    match BETA.get(ordinal) {
        Some(b) => *b,
        None => (2.0 * ordinal as f64 + 3.0) * std::f64::consts::FRAC_PI_2,
    }
}

/// Fletcher & Rossing 2e, sec. 2.19-2.20.
pub fn modes(g: &BarGeom, count: usize) -> Vec<Mode> {
    let radius_of_gyration = g.thickness / 12f64.sqrt();
    let bar_speed = (g.young / g.density).sqrt();
    let scale = std::f64::consts::PI * radius_of_gyration * bar_speed / (8.0 * g.length * g.length);
    (0..count)
        .map(|ordinal| mode(scale * beta(ordinal).powi(2), 1.0, g.damping))
        .collect()
}
