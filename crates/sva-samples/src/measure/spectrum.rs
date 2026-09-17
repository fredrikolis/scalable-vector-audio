// Concern: resolves samples into magnitudes, third-octave bands and peaks | Non-concern: naming a peak as a note (pitch.rs), choosing the window | IO: (&[f64], sample rate) -> Spectrum

use crate::fft::fft;

const MAX_FRAME: usize = 8192;
const MIN_FRAME: usize = 64;

/// One FFT's memory, which is all that bounds a caller's own frame.
pub const MAX_PINNED_FRAME: usize = 1 << 20;

/// A caller checks this against [`MAX_PINNED_FRAME`]: a shrunk frame is not a pinned one.
pub fn pinned_frame(secs: f64, sample_rate: f64) -> usize {
    let asked = (secs * sample_rate).round().max(0.0);
    if !asked.is_finite() || asked > MAX_PINNED_FRAME as f64 {
        return MAX_PINNED_FRAME * 2;
    }
    (asked as usize).next_power_of_two().max(MIN_FRAME)
}
const FLOOR_DB: f64 = -160.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub db: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub hz: f64,
    pub db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spectrum {
    pub frame_size: usize,
    pub frames: usize,
    pub resolution_hz: f64,
    pub rms: f64,
    pub centroid_hz: f64,
    pub rolloff85_hz: f64,
    pub bands: Vec<Band>,
    pub peaks: Vec<Peak>,
}

pub fn analyze(
    samples: &[f64],
    sample_rate: f64,
    max_peaks: usize,
    frame_secs: Option<f64>,
) -> Spectrum {
    let (mags, bin_hz, frame_size, frames) = magnitudes(samples, sample_rate, frame_secs);
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

    let mut running = 0.0;
    let mut rolloff85_hz = 0.0;
    for (k, p) in power.iter().enumerate() {
        running += p;
        if total > 0.0 && running >= 0.85 * total {
            rolloff85_hz = k as f64 * bin_hz;
            break;
        }
    }

    let rms = if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|&x| x * x).sum::<f64>() / samples.len() as f64).sqrt()
    };

    Spectrum {
        frame_size,
        frames,
        resolution_hz: bin_hz,
        rms,
        centroid_hz,
        rolloff85_hz,
        bands: bands(&power, bin_hz, sample_rate),
        peaks: peaks(&mags, bin_hz, max_peaks),
    }
}

/// Welch-averaged, Hann-windowed magnitudes normalized so a full-scale sine reads 1.0 at its
/// own bin. Without `frame_secs` the size follows the WINDOW, so one node's two windows land
/// on two bin grids and cannot be compared.
pub fn magnitudes(
    samples: &[f64],
    sample_rate: f64,
    frame_secs: Option<f64>,
) -> (Vec<f64>, f64, usize, usize) {
    let frame_size = match frame_secs {
        Some(secs) => pinned_frame(secs, sample_rate).min(MAX_PINNED_FRAME),
        None => samples
            .len()
            .next_power_of_two()
            .clamp(MIN_FRAME, MAX_FRAME),
    };
    let hop = frame_size / 2;
    let bins = frame_size / 2 + 1;
    let window = crate::stft::hann_periodic(frame_size);

    let mut acc = vec![0f64; bins];
    let mut frames = 0usize;
    let mut start = 0usize;
    loop {
        let mut re = vec![0f64; frame_size];
        let mut im = vec![0f64; frame_size];
        for n in 0..frame_size {
            let s = samples.get(start + n).copied().unwrap_or(0.0);
            re[n] = s * window[n];
        }
        fft(&mut re, &mut im);
        for (k, a) in acc.iter_mut().enumerate() {
            *a += re[k] * re[k] + im[k] * im[k];
        }
        frames += 1;
        start += hop;
        if start + frame_size > samples.len() {
            break;
        }
    }

    let scale = 4.0 / frame_size as f64;
    let mags = acc
        .iter()
        .map(|p| (p / frames as f64).sqrt() * scale)
        .collect();
    (mags, sample_rate / frame_size as f64, frame_size, frames)
}

pub fn db(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * amplitude.log10()).max(FLOOR_DB)
    }
}

/// Third-octave from 20 Hz up, the resolution a listener's critical bands roughly follow.
pub fn third_octave_edges(sample_rate: f64) -> Vec<(f64, f64)> {
    let nyquist = sample_rate / 2.0;
    let ratio = 2f64.powf(1.0 / 3.0);
    let mut out = Vec::new();
    let mut lo = 20.0;
    while lo < nyquist {
        let hi = (lo * ratio).min(nyquist);
        out.push((lo, hi));
        lo = hi;
    }
    out
}

fn bands(power: &[f64], bin_hz: f64, sample_rate: f64) -> Vec<Band> {
    let mut out = Vec::new();
    for (lo, hi) in third_octave_edges(sample_rate) {
        let sum: f64 = power
            .iter()
            .enumerate()
            .filter(|(k, _)| {
                let hz = *k as f64 * bin_hz;
                hz >= lo && hz < hi
            })
            .map(|(_, p)| p)
            .sum();
        out.push(Band {
            lo_hz: lo,
            hi_hz: hi,
            db: db(sum.sqrt()),
        });
    }
    out
}

/// Local maxima, parabolically interpolated so a peak between two bins reports its real
/// frequency instead of the nearest bin's.
pub fn peaks(mags: &[f64], bin_hz: f64, max_peaks: usize) -> Vec<Peak> {
    let mut found: Vec<Peak> = Vec::new();
    for k in 1..mags.len().saturating_sub(1) {
        if mags[k] <= mags[k - 1] || mags[k] < mags[k + 1] || mags[k] <= 0.0 {
            continue;
        }
        let (a, b, c) = (mags[k - 1], mags[k], mags[k + 1]);
        let denom = a - 2.0 * b + c;
        let delta = if denom.abs() > f64::EPSILON {
            (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        found.push(Peak {
            hz: (k as f64 + delta) * bin_hz,
            db: db(b - 0.25 * (a - c) * delta),
        });
    }
    found.sort_by(|x, y| y.db.total_cmp(&x.db));
    found.truncate(max_peaks);
    found
}
