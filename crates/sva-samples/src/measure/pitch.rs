// Concern: names each frame's spectral peaks as notes and flags likely harmonics | Non-concern: computing the spectrum (spectrum.rs), chord or key identity | IO: (&[f64], sample rate) -> frames

use crate::measure::spectrum::{Peak, magnitudes, peaks};

const NAMES: [&str; 12] = [
    "C", "Cs", "D", "Ds", "E", "F", "Fs", "G", "Gs", "A", "As", "B",
];
const LOWEST_HZ: f64 = 25.0;
const HIGHEST_HZ: f64 = 5000.0;
const HARMONIC_TOLERANCE: f64 = 0.04;
const HIGHEST_HARMONIC: f64 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub hz: f64,
    pub name: String,
    pub cents: f64,
    pub db: f64,
    pub harmonic_of: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PitchFrame {
    pub t_secs: f64,
    pub notes: Vec<Note>,
}

pub fn track(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    frame_secs: f64,
    max_notes: usize,
) -> Vec<PitchFrame> {
    let stride = ((frame_secs * sample_rate).round() as usize).max(1);
    samples
        .chunks(stride)
        .enumerate()
        .map(|(n, chunk)| {
            let (mags, bin_hz, ..) = magnitudes(chunk, sample_rate, None);
            let found = peaks(&mags, bin_hz, max_notes * 4);
            PitchFrame {
                t_secs: start_secs + (n * stride) as f64 / sample_rate,
                notes: name_all(&found, max_notes),
            }
        })
        .collect()
}

/// The same naming over peaks a reading already holds exactly, loudest first.
pub fn name_peaks(found: &[Peak], max_notes: usize) -> Vec<Note> {
    name_all(found, max_notes)
}

/// A peak sitting at a near-integer multiple of a LOUDER peak is reported as that peak's
/// harmonic, so a saw's partials never read as a chord of their own.
fn name_all(found: &[Peak], max_notes: usize) -> Vec<Note> {
    let audible: Vec<&Peak> = found
        .iter()
        .filter(|p| (LOWEST_HZ..=HIGHEST_HZ).contains(&p.hz))
        .collect();

    audible
        .iter()
        .take(max_notes)
        .map(|p| {
            let midi = 69.0 + 12.0 * (p.hz / 440.0).log2();
            let nearest = midi.round();
            Note {
                hz: p.hz,
                name: format!(
                    "{}{}",
                    NAMES[(nearest as i64).rem_euclid(12) as usize],
                    (nearest as i64) / 12 - 1
                ),
                cents: (midi - nearest) * 100.0,
                db: p.db,
                harmonic_of: harmonic_of(p, &audible),
            }
        })
        .collect()
}

fn harmonic_of(peak: &Peak, louder_first: &[&Peak]) -> Option<f64> {
    louder_first
        .iter()
        .take_while(|other| other.db > peak.db)
        .find(|other| {
            let n = peak.hz / other.hz;
            (1.5..=HIGHEST_HARMONIC).contains(&n) && (n - n.round()).abs() < HARMONIC_TOLERANCE
        })
        .map(|other| other.hz)
}
