// Concern: an air column's eigenfrequencies with its end correction | Non-concern: tone holes | IO: (BoreGeom, count) -> Vec<Mode>

use crate::closed_form::Mode;
use crate::modal::damping::Damping;
use crate::modal::mode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ends {
    OpenOpen,
    ClosedOpen,
}

pub struct BoreGeom {
    pub length: f64,
    pub radius: f64,
    pub speed: f64,
    pub ends: Ends,
    pub damping: Damping,
}

/// Unflanged; Levine & Schwinger, Phys. Rev. 73 (1948) 383.
pub const END_CORRECTION: f64 = 0.613_3;

pub fn correction(radius: f64) -> f64 {
    END_CORRECTION * radius
}

pub fn modes(g: &BoreGeom, count: usize) -> Vec<Mode> {
    let dl = correction(g.radius);
    (1..=count)
        .map(|n| {
            let n = n as f64;
            let hz = match g.ends {
                Ends::OpenOpen => n * g.speed / (2.0 * (g.length + 2.0 * dl)),
                Ends::ClosedOpen => (2.0 * n - 1.0) * g.speed / (4.0 * (g.length + dl)),
            };
            mode(hz, 1.0, g.damping)
        })
        .collect()
}

/// Kinsler & Frey, Fundamentals of Acoustics 4e, sec. 10.5.
pub fn helmholtz(volume: f64, neck_area: f64, neck_length: f64, radius: f64, speed: f64) -> f64 {
    let effective = neck_length + 1.7 * radius;
    speed / std::f64::consts::TAU * (neck_area / (volume * effective)).sqrt()
}
