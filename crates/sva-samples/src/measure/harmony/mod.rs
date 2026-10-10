// Concern: the partials a harmonic reading starts from, per instant or frame, and their floor | Non-concern: what a reading makes of them (siblings) | IO: (lines) -> partials

pub mod chords;
pub mod dissonance;
pub mod measured;
pub mod tonnetz;
pub mod voice_leading;

use std::collections::HashMap;

use sva_formula::C64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Partial {
    pub hz: f64,
    pub amplitude: f64,
    /// Where the true values lie, `(lo, hi)`; a point where the partial is exact.
    pub hz_bounds: (f64, f64),
    pub amplitude_bounds: (f64, f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Moment {
    pub t_secs: f64,
    pub partials: Vec<Partial>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub hop_secs: f64,
    pub window: Option<Window>,
}

impl Framing {
    pub fn method(&self) -> &'static str {
        match self.window {
            None => EXACT_METHOD,
            Some(_) => measured::METHOD,
        }
    }
}

pub const EXACT_METHOD: &str = "exact: at each instant every atom of the closed form's first \
                                component is a partial at its carrier frequency, its value \
                                there summed with the others at that frequency, and a cosine's \
                                two conjugate lines one partial";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub size: usize,
    pub secs: f64,
    pub resolution_hz: f64,
}

/// Lines at one frequency sum as complex amplitudes, so a cancelled pair is no partial; a
/// real wave's amplitude is its two conjugate lines' moduli summed.
pub fn partials(lines: impl IntoIterator<Item = (f64, C64)>) -> Vec<Partial> {
    let mut at: HashMap<u64, C64> = HashMap::new();
    let mut order: Vec<f64> = Vec::new();
    for (hz, amp) in lines {
        let summed = at.entry(hz.to_bits()).or_insert_with(|| {
            order.push(hz);
            C64::ZERO
        });
        *summed = *summed + amp;
    }
    let modulus = |hz: f64| at.get(&hz.to_bits()).map_or(0.0, |c| c.abs());
    order
        .into_iter()
        .filter(|hz| *hz > 0.0)
        .map(|hz| {
            let amplitude = modulus(hz) + modulus(-hz);
            Partial {
                hz,
                amplitude,
                hz_bounds: (hz, hz),
                amplitude_bounds: (amplitude, amplitude),
            }
        })
        .filter(|p| p.amplitude > 0.0)
        .collect()
}

pub fn above(moment: &Moment, floor_db: f64) -> Vec<Partial> {
    let floor = 10f64.powf(floor_db / 20.0);
    moment
        .partials
        .iter()
        .filter(|p| p.amplitude >= floor)
        .copied()
        .collect()
}
