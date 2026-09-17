// Concern: a stiff string's inharmonic eigenfrequencies and per-mode decay | Non-concern: the excitation multiplying them (mod.rs) | IO: (StringGeom, count) -> Vec<Mode>

use crate::closed_form::Mode;
use crate::modal::damping::Damping;
use crate::modal::mode;

pub struct StringGeom {
    pub f0: f64,
    pub inharmonicity: f64,
    pub strike: f64,
    pub damping: Damping,
}

/// Fletcher & Rossing 2e, sec. 2.18, after Fletcher, JASA 36 (1964) 203.
pub fn inharmonicity(young: f64, diameter: f64, tension: f64, length: f64) -> f64 {
    std::f64::consts::PI.powi(3) * young * diameter.powi(4) / (64.0 * tension * length * length)
}

/// The struck-string amplitude is Fletcher & Rossing 2e, sec. 2.6.
pub fn modes(g: &StringGeom, count: usize) -> Vec<Mode> {
    (1..=count)
        .map(|n| {
            let n = n as f64;
            let hz = n * g.f0 * (1.0 + g.inharmonicity * n * n).sqrt();
            let amp = (n * std::f64::consts::PI * g.strike).sin() / (n * n);
            mode(hz, amp, g.damping)
        })
        .collect()
}
