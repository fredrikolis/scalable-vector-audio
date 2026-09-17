// Concern: a rectangular room's axial, tangential and oblique eigenfrequencies | Non-concern: absorption per surface | IO: (RoomGeom, count) -> Vec<Mode>

use crate::closed_form::Mode;
use crate::modal::damping::Damping;
use crate::modal::grid::{lowest, reach};
use crate::modal::mode;

pub struct RoomGeom {
    pub lx: f64,
    pub ly: f64,
    pub lz: f64,
    pub speed: f64,
    pub damping: Damping,
}

/// Kuttruff, Room Acoustics 5e, sec. 3.1; one nonzero axis more halves the amplitude.
pub fn modes(g: &RoomGeom, count: usize) -> Vec<Mode> {
    let span = reach(&[g.lx, g.ly, g.lz], count);
    let mut found = Vec::new();
    for l in 0..=span[0] {
        for m in 0..=span[1] {
            for n in 0..=span[2] {
                let axes = [l, m, n].iter().filter(|k| **k > 0).count();
                if axes == 0 {
                    continue;
                }
                let hz = 0.5
                    * g.speed
                    * ((l as f64 / g.lx).powi(2)
                        + (m as f64 / g.ly).powi(2)
                        + (n as f64 / g.lz).powi(2))
                    .sqrt();
                found.push(mode(hz, 0.5f64.powi(axes as i32 - 1), g.damping));
            }
        }
    }
    lowest(found, count)
}
