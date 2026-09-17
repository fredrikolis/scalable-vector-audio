// Concern: per-frame trajectory and a tolerance-based monotonicity verdict | Non-concern: computing level or width (envelope.rs/stereo.rs) | IO: (frames, samples) -> Trajectory

use sva_samples::{EnvelopeFrame, StereoImage};

use crate::decibels::to_db;
use crate::frame::spectral_frames;

/// Loudness's just-noticeable difference sits near 1 dB; half of that absorbs windowing and
/// quantization jitter in an RMS trace without hiding a real, audible decay.
pub const LEVEL_TOLERANCE_DB: f64 = 0.5;
/// The same slack for a linear ratio (width) rather than a level.
pub const RATIO_TOLERANCE: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrajectoryFrame {
    pub t_secs: f64,
    pub rms_db: f64,
    pub peak_db: f64,
    pub width: Option<f64>,
    pub centroid_hz: f64,
    pub flatness: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Rising,
    Falling,
    Flat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub monotonic: bool,
    pub direction: Direction,
    pub violations: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trajectory {
    pub frames: Vec<TrajectoryFrame>,
    pub level: Option<Verdict>,
    pub width: Option<Verdict>,
    pub centroid: Option<Verdict>,
}

pub fn analyze(
    envelope: &[EnvelopeFrame],
    stereo: Option<&StereoImage>,
    samples: &[f32],
    sample_rate: f64,
    frame_secs: f64,
) -> Trajectory {
    let start_secs = envelope.first().map_or(0.0, |f| f.t_secs);
    let spectral = spectral_frames(samples, sample_rate, start_secs, frame_secs, frame_secs);
    let frames: Vec<TrajectoryFrame> = envelope
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (centroid_hz, flatness) = spectral
                .get(i)
                .map(|f| spectral_measures(&f.mags, f.bin_hz))
                .unwrap_or((0.0, 0.0));
            TrajectoryFrame {
                t_secs: e.t_secs,
                rms_db: to_db(e.rms),
                peak_db: to_db(e.peak),
                width: stereo.and_then(|s| s.frames.get(i)).map(|f| f.width),
                centroid_hz,
                flatness,
            }
        })
        .collect();

    let level = verdict(
        &frames
            .iter()
            .map(|f| (f.t_secs, f.rms_db))
            .collect::<Vec<_>>(),
        Tolerance::Absolute(LEVEL_TOLERANCE_DB),
    );
    let width = stereo.and_then(|_| {
        verdict(
            &frames
                .iter()
                .filter_map(|f| f.width.map(|w| (f.t_secs, w)))
                .collect::<Vec<_>>(),
            Tolerance::Relative(RATIO_TOLERANCE),
        )
    });
    let centroid = verdict(
        &frames
            .iter()
            .map(|f| (f.t_secs, f.centroid_hz))
            .collect::<Vec<_>>(),
        Tolerance::Relative(RATIO_TOLERANCE),
    );

    Trajectory {
        frames,
        level,
        width,
        centroid,
    }
}

/// Centroid as `spectrum::analyze` computes it; flatness is the Wiener entropy — the power
/// spectrum's geometric mean over its arithmetic mean, 1.0 for white noise and near 0 for a
/// single tone.
fn spectral_measures(mags: &[f64], bin_hz: f64) -> (f64, f64) {
    let power: Vec<f64> = mags.iter().map(|m| m * m).collect();
    let total: f64 = power.iter().sum();
    let centroid_hz = if total > 0.0 {
        power
            .iter()
            .enumerate()
            .map(|(k, p)| k as f64 * bin_hz * p)
            .sum::<f64>()
            / total
    } else {
        0.0
    };
    let floor = 1e-12;
    let n = power.len().max(1) as f64;
    let log_mean = power.iter().map(|p| (p + floor).ln()).sum::<f64>() / n;
    let arithmetic_mean = (total + floor) / n;
    let flatness = (log_mean.exp() / arithmetic_mean).clamp(0.0, 1.0);
    (centroid_hz, flatness)
}

#[derive(Clone, Copy)]
enum Tolerance {
    Absolute(f64),
    Relative(f64),
}

/// Tracks a running extreme in the claimed direction; a frame outside a slack band around it
/// (additive in dB, multiplicative for a ratio) is a violation, and resets the extreme there so
/// one real step does not keep re-triggering. A step of `-inf` to `-inf` — a window silent
/// throughout, or none at all — is the one head-to-tail difference with no sign to read.
fn verdict(points: &[(f64, f64)], tolerance: Tolerance) -> Option<Verdict> {
    let span = (points.len() / 5).max(1).min(points.len());
    let head = mean(&points[..span]);
    let tail = mean(&points[points.len() - span..]);
    if (tail - head).is_nan() {
        return None;
    }
    let flat_band = match tolerance {
        Tolerance::Absolute(t) => t,
        Tolerance::Relative(t) => head.abs() * t,
    };
    let direction = if (tail - head).abs() <= flat_band {
        Direction::Flat
    } else if tail > head {
        Direction::Rising
    } else {
        Direction::Falling
    };

    let rising = matches!(direction, Direction::Rising);
    let mut extreme = points[0].1;
    let mut violations = Vec::new();
    for &(t, v) in &points[1..] {
        let ok = match (direction, tolerance) {
            (Direction::Flat, Tolerance::Absolute(tol)) => (v - points[0].1).abs() <= tol,
            (Direction::Flat, Tolerance::Relative(tol)) => {
                (v - points[0].1).abs() <= points[0].1.abs() * tol
            }
            (_, Tolerance::Absolute(tol)) if rising => v >= extreme - tol,
            (_, Tolerance::Absolute(tol)) => v <= extreme + tol,
            (_, Tolerance::Relative(tol)) if rising => v >= extreme * (1.0 - tol),
            (_, Tolerance::Relative(tol)) => v <= extreme * (1.0 + tol),
        };
        if ok {
            extreme = if rising {
                extreme.max(v)
            } else {
                extreme.min(v)
            };
        } else {
            violations.push(t);
            extreme = v;
        }
    }
    Some(Verdict {
        monotonic: violations.is_empty(),
        direction,
        violations,
    })
}

fn mean(points: &[(f64, f64)]) -> f64 {
    points.iter().map(|&(_, v)| v).sum::<f64>() / points.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(rms: &[f64]) -> Vec<EnvelopeFrame> {
        rms.iter()
            .enumerate()
            .map(|(i, &r)| EnvelopeFrame {
                t_secs: i as f64 * 0.01,
                rms: r,
                peak: r,
            })
            .collect()
    }

    #[test]
    fn a_clean_decay_reads_monotonic_falling() {
        let e = envelope(&[1.0, 0.5, 0.25, 0.125, 0.0625, 0.03125]);
        let t = analyze(&e, None, &vec![0.0f32; 600], 44100.0, 0.01);
        let level = t.level.expect("a sounding window has a level verdict");
        assert_eq!(level.direction, Direction::Falling);
        assert!(level.monotonic, "{:?}", level.violations);
    }

    /// The whole point of a tolerance band: real audio jitters by a fraction of a dB frame to
    /// frame even while genuinely decaying, and a strict `<=` would flag every one of those.
    #[test]
    fn a_decay_with_natural_micro_variation_still_reads_monotonic() {
        let mut rms = vec![1.0];
        for i in 1..40 {
            let trend = 1.0 * 0.9f64.powi(i);
            let jitter = if i % 2 == 0 { 1.02 } else { 0.99 };
            rms.push(trend * jitter);
        }
        let e = envelope(&rms);
        let t = analyze(&e, None, &vec![0.0f32; 4000], 44100.0, 0.01);
        let level = t.level.expect("a sounding window has a level verdict");
        assert!(level.monotonic, "{:?}", level.violations);
        assert_eq!(level.direction, Direction::Falling);
    }

    #[test]
    fn a_real_swell_in_the_middle_of_a_decay_is_flagged() {
        let mut rms = vec![1.0, 0.8, 0.6, 0.4];
        rms.extend([0.9, 0.85]);
        rms.extend([0.3, 0.2, 0.1, 0.05]);
        let e = envelope(&rms);
        let t = analyze(&e, None, &vec![0.0f32; 1000], 44100.0, 0.01);
        let level = t.level.expect("a sounding window has a level verdict");
        assert!(!level.monotonic);
        assert!(!level.violations.is_empty());
    }

    #[test]
    fn a_steady_level_reads_flat_not_falling() {
        let e = envelope(&[0.5; 20]);
        let t = analyze(&e, None, &vec![0.0f32; 2000], 44100.0, 0.01);
        let level = t.level.expect("a sounding window has a level verdict");
        assert_eq!(level.direction, Direction::Flat);
        assert!(level.monotonic);
    }

    #[test]
    fn width_is_absent_without_a_stereo_image() {
        let e = envelope(&[0.5, 0.4]);
        let t = analyze(&e, None, &vec![0.0f32; 200], 44100.0, 0.01);
        assert!(t.width.is_none());
    }
}
