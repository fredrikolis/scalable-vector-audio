// Concern: names each frame's spectral peaks as notes and flags likely harmonics | Non-concern: computing the spectrum (spectrum.rs), chord or key identity | IO: (&[f64], sample rate) -> frames

use crate::measure::spectrum::{Peak, magnitudes, peaks};

const NAMES: [&str; 12] = [
    "C", "Cs", "D", "Ds", "E", "F", "Fs", "G", "Gs", "A", "As", "B",
];
pub const LOWEST_HZ: f64 = 25.0;
pub const HIGHEST_HZ: f64 = 5000.0;
const HARMONIC_TOLERANCE: f64 = 0.04;
const HIGHEST_HARMONIC: f64 = 8.0;
const COMPRESSION: f64 = 0.84;

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

/// A note at a near-whole multiple of a lower one in its series is reported as its harmonic.
fn name_all(found: &[Peak], max_notes: usize) -> Vec<Note> {
    let listed: Vec<&Peak> = found
        .iter()
        .filter(|p| (LOWEST_HZ..=HIGHEST_HZ).contains(&p.hz))
        .take(max_notes)
        .collect();
    let roots = series_roots(&listed);

    listed
        .iter()
        .zip(roots)
        .map(|(p, harmonic_of)| {
            let midi = midi_of(p.hz);
            let nearest = midi.round();
            Note {
                hz: p.hz,
                name: note_name(nearest as i64),
                cents: (midi - nearest) * 100.0,
                db: p.db,
                harmonic_of,
            }
        })
        .collect()
}

pub fn midi_of(hz: f64) -> f64 {
    69.0 + 12.0 * (hz / 440.0).log2()
}

pub fn note_name(midi: i64) -> String {
    format!("{}{}", class_name(midi), midi.div_euclid(12) - 1)
}

pub fn class_name(midi: i64) -> &'static str {
    NAMES[midi.rem_euclid(12) as usize]
}

/// Klapuri's iterative cancellation (ISMIR 2006): the note with the greatest subharmonic sum
/// (Hermes, JASA 83(1), 1988) roots a series, and the rest go round again. A note at 2 to 8
/// times a louder member joins it, or the root itself if their pair's sum at the root is at
/// least the note's own amplitude: so a weak fundamental heads its octave.
fn series_roots(notes: &[&Peak]) -> Vec<Option<f64>> {
    let mut joined = vec![None; notes.len()];
    let mut left: Vec<usize> = (0..notes.len()).collect();
    while let Some(root) = most_salient(notes, &left) {
        left.retain(|&i| i != root);
        let joins = |m: usize, i: usize| {
            multiple(notes[i].hz, notes[m].hz).is_some_and(|k| {
                let (under, over) = (amp(notes[m].db), amp(notes[i].db));
                k >= 2
                    && (under > over
                        || m == root && under + COMPRESSION.powi(k as i32 - 1) * over >= over)
            })
        };
        let mut series = vec![root];
        while let Some((at, m)) = left
            .iter()
            .enumerate()
            .find_map(|(at, &i)| series.iter().find(|&&m| joins(m, i)).map(|&m| (at, m)))
        {
            let i = left.remove(at);
            joined[i] = Some(notes[m].hz);
            series.push(i);
        }
    }
    joined
}

fn amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn most_salient(notes: &[&Peak], left: &[usize]) -> Option<usize> {
    let salience = |c: usize| {
        let mut strongest = [0.0f64; HIGHEST_HARMONIC as usize];
        for &i in left {
            if let Some(k) = multiple(notes[i].hz, notes[c].hz) {
                let at = &mut strongest[k as usize - 1];
                *at = at.max(amp(notes[i].db));
            }
        }
        strongest
            .iter()
            .enumerate()
            .map(|(k, a)| COMPRESSION.powi(k as i32) * a)
            .sum::<f64>()
    };
    left.iter()
        .map(|&c| (c, salience(c)))
        .max_by(|(a, sa), (b, sb)| sa.total_cmp(sb).then(notes[*b].hz.total_cmp(&notes[*a].hz)))
        .map(|(c, _)| c)
}

fn multiple(hz: f64, root: f64) -> Option<u32> {
    let n = hz / root;
    let k = n.round();
    ((1.0..=HIGHEST_HARMONIC).contains(&k) && (n - k).abs() < HARMONIC_TOLERANCE)
        .then_some(k as u32)
}
