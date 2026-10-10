// Concern: how strongly onset strength repeats per sliding window: broadband, low, rest | Non-concern: the band filters (bands.rs), any verdict | IO: (&[f64], rate, start, hop, window) -> Periodicity

use super::bands::{self, Bands, Smoothing};
use super::harmony::dissonance::Extreme;
use super::onsets::spectral_flux;
use crate::fft::{fft, ifft};

/// A band belongs to `low` where its centre is under this; kick and sub sit there.
pub const SPLIT_HZ: f64 = 150.0;
/// 600 bpm, a sixteenth at 150 bpm.
pub const MIN_PERIOD_SECS: f64 = 0.1;
/// 30 bpm.
pub const MAX_PERIOD_SECS: f64 = 2.0;
/// Every lag read then overlaps at least half its window.
pub const MIN_WINDOW_SECS: f64 = 2.0 * MAX_PERIOD_SECS;
pub const PEAKS: usize = 3;
/// Each band's energy through two one-poles at 10 Hz before it is decimated: a sustained
/// tone's ripple at twice its frequency, which would alias into the envelope, falls 12 dB an
/// octave and no longer outweighs a slow swell's rise.
pub const SMOOTHING: Smoothing = Smoothing {
    hz: Some(10.0),
    poles: 2,
};

pub const MODEL: &str = "onset strength: the spectral flux `onsets` takes, sum over a group's \
                         bands of max(0, e[n] - e[n-1]), run over the ERB bands of `bands` \
                         (Glasberg and Moore's filter widths), e each band's RMS envelope at \
                         `envelope_hz`, its energy smoothed before decimation by \
                         `smoothing.poles` one-pole lowpasses at `smoothing.hz` in place of the \
                         one at the band's own width `bands` reads through, so an onset is \
                         spread over some 30 ms; linear amplitude so a loud band outweighs a quiet one, \
                         from the filters' silent start; the first component alone. A band is \
                         `low` where its centre is under `split_hz` and `rest` otherwise; \
                         `broadband` sums every band. The filters' skirts overlap, so a kick \
                         leaks into `rest` and a hat's attack into `low`, attenuated. That \
                         filterbank, smoothed envelope and rectified difference is Scheirer, E. D. \
                         (1998), Tempo and beat analysis of acoustic musical signals, J. Acoust. \
                         Soc. Am. 103(1) 588-601. Each window is `window_secs` of that envelope \
                         centred on t, t stepping by `hop_secs` from the target's start as \
                         every framed reading steps, clipped to the target at its ends, \
                         `from`/`to` its span; its mean is subtracted, y. height(tau) = sum over \
                         n < W - tau of y[n] y[n + tau] over sum of y[n]^2, the autocorrelation \
                         of the onset envelope Ellis, D. P. W. (2007), Beat tracking by dynamic \
                         programming, J. New Music Res. 36(1) 51-60, reads tempo from, \
                         normalized at lag 0 and biased: |height| <= 1 by Cauchy-Schwarz, \
                         taken by FFT and exact up to its rounding, and an envelope repeating \
                         exactly every tau peaks near 1 - tau/W, so of two periods that repeat \
                         equally the shorter reads higher, and heights at different periods or \
                         spans are not on one scale. peaks: lags from `min_period_secs` to \
                         `max_period_secs` whose height is positive, over the lag before and \
                         no lower than the lag after, the highest three, highest first; a \
                         period is that lag on the envelope's grid, `lag_step_secs` apart, with \
                         no interpolation between lags; bpm = 60 / period. No tempo prior (Ellis \
                         weights by a log-Gaussian) and no octave choice: a beat, its half and \
                         its double may each lead, and each is reported; no inter-onset \
                         clustering or beat tracking (Dixon, S. (2001), Automatic extraction \
                         of tempo and beat from expressive performances, J. New Music Res. \
                         30(1) 39-58) is done. drift_bpm: the leading peak's bpm less the \
                         previous window's, null where either has none, so a lead jumping \
                         between a beat and its double reads as a large drift. curve, where \
                         asked: the height at every lag from `min_period_secs` by \
                         `lag_step_secs` to the window's longest; heights and curves are null \
                         where the window's envelope is constant. summary: over the windows \
                         with a peak, the leading peak's bpm and height and the drift, their \
                         mean, min and max with the t each stands at. Exact path: none; a \
                         closed form, its onsets placed exactly or not, is read off its \
                         rendered samples like any other, because the onset strength is the \
                         band envelopes' rise, which no closed form states";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Broadband,
    Low,
    Rest,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Broadband, Group::Low, Group::Rest];

    pub fn key(self) -> &'static str {
        match self {
            Group::Broadband => "broadband",
            Group::Low => "low",
            Group::Rest => "rest",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub period_secs: f64,
    pub bpm: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub t_secs: f64,
    pub from_secs: f64,
    pub to_secs: f64,
    /// Highest first.
    pub peaks: Vec<Peak>,
    pub drift_bpm: Option<f64>,
    /// `None` where not asked; NaN at each lag where the envelope is constant.
    pub curve: Option<Vec<f64>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    pub mean: f64,
    pub min: Extreme,
    pub max: Extreme,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub windows_with_peak: usize,
    pub bpm: Option<Stats>,
    pub height: Option<Stats>,
    pub drift_bpm: Option<Stats>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub group: Group,
    pub centres_hz: Vec<f64>,
    pub windows: Vec<Window>,
    pub summary: Summary,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Periodicity {
    pub hop_secs: f64,
    pub window_secs: f64,
    pub envelope_hz: f64,
    pub lag_step_secs: f64,
    pub min_period_secs: f64,
    pub max_period_secs: f64,
    pub tracks: Vec<Track>,
}

/// Why no window was read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unread {
    WindowUnderTwoPeriods { window_secs: f64 },
    TargetShorterThanWindow { held_secs: f64, window_secs: f64 },
}

pub fn measure(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    hop_secs: f64,
    window_secs: f64,
    curve: bool,
) -> Result<Periodicity, Unread> {
    if window_secs < MIN_WINDOW_SECS {
        return Err(Unread::WindowUnderTwoPeriods { window_secs });
    }
    let held_secs = samples.len() as f64 / sample_rate;
    if held_secs < window_secs {
        return Err(Unread::TargetShorterThanWindow {
            held_secs,
            window_secs,
        });
    }
    let split = bands::smoothed(samples, sample_rate, start_secs, SMOOTHING);
    let rate = split.rate_hz;
    let lags = Lags {
        min: (MIN_PERIOD_SECS * rate).ceil() as usize,
        max: (MAX_PERIOD_SECS * rate).floor() as usize,
        rate,
    };
    let hop = ((hop_secs * sample_rate).round() as usize).max(1);
    let size = (window_secs * rate).round() as usize;
    let centres: Vec<usize> = (0..samples.len()).step_by(hop).collect();
    let tracks = Group::ALL
        .into_iter()
        .zip(envelopes(&split))
        .map(|(group, (centres_hz, envelope))| {
            let mut windows: Vec<Window> = centres
                .iter()
                .map(|&c| {
                    let middle = (c as f64 * rate / sample_rate).round() as usize;
                    let from = middle.saturating_sub(size / 2);
                    let to = (middle + size - size / 2).min(envelope.len());
                    let heights = heights(&envelope[from..to], lags.max + 1);
                    Window {
                        t_secs: start_secs + c as f64 / sample_rate,
                        from_secs: start_secs + from as f64 / rate,
                        to_secs: start_secs + to as f64 / rate,
                        peaks: lags.peaks(&heights),
                        drift_bpm: None,
                        curve: curve.then(|| {
                            heights[..heights.len().min(lags.max + 1)]
                                .iter()
                                .skip(lags.min)
                                .copied()
                                .collect()
                        }),
                    }
                })
                .collect();
            for i in 1..windows.len() {
                let lead = |w: &Window| w.peaks.first().map(|p| p.bpm);
                windows[i].drift_bpm = lead(&windows[i])
                    .zip(lead(&windows[i - 1]))
                    .map(|(a, b)| a - b);
            }
            Track {
                group,
                centres_hz,
                summary: summary(&windows),
                windows,
            }
        })
        .collect();
    Ok(Periodicity {
        hop_secs: hop as f64 / sample_rate,
        window_secs: size as f64 / rate,
        envelope_hz: rate,
        lag_step_secs: 1.0 / rate,
        min_period_secs: lags.min as f64 / rate,
        max_period_secs: lags.max as f64 / rate,
        tracks,
    })
}

/// Each group's band centres and onset-strength envelope, in `Group::ALL`'s order.
fn envelopes(split: &Bands) -> Vec<(Vec<f64>, Vec<f64>)> {
    let tracks = &split.bands;
    let low = tracks
        .iter()
        .position(|b| b.centre_hz >= SPLIT_HZ)
        .unwrap_or(tracks.len());
    let ranges = [0..tracks.len(), 0..low, low..tracks.len()];
    let len = tracks.first().map_or(0, |b| b.rms.len());
    let mut flux: Vec<Vec<f64>> = ranges.iter().map(|_| vec![0.0; len]).collect();
    let (mut prev, mut curr) = (vec![0.0; tracks.len()], vec![0.0; tracks.len()]);
    for n in 0..len {
        for (held, band) in curr.iter_mut().zip(tracks) {
            *held = band.rms[n];
        }
        if n > 0 {
            for (out, range) in flux.iter_mut().zip(&ranges) {
                out[n] = spectral_flux(&prev[range.clone()], &curr[range.clone()]);
            }
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    ranges
        .into_iter()
        .zip(flux)
        .map(|(range, envelope)| {
            let centres = tracks[range].iter().map(|b| b.centre_hz).collect();
            (centres, envelope)
        })
        .collect()
}

/// The mean-removed segment's autocorrelation at lags `0..=top` the segment holds, over its
/// value at lag 0; zero-padded past `len + top` so no lag wraps.
fn heights(segment: &[f64], top: usize) -> Vec<f64> {
    let top = top.min(segment.len().saturating_sub(1));
    let mean = segment.iter().sum::<f64>() / segment.len() as f64;
    let n = (segment.len() + top + 1).next_power_of_two();
    let mut re = vec![0.0; n];
    for (y, x) in re.iter_mut().zip(segment) {
        *y = x - mean;
    }
    let energy: f64 = re.iter().map(|y| y * y).sum();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    for (r, i) in re.iter_mut().zip(im.iter_mut()) {
        *r = *r * *r + *i * *i;
        *i = 0.0;
    }
    ifft(&mut re, &mut im);
    re.truncate(top + 1);
    re.into_iter().map(|r| r / energy).collect()
}

struct Lags {
    min: usize,
    max: usize,
    rate: f64,
}

impl Lags {
    fn peaks(&self, heights: &[f64]) -> Vec<Peak> {
        let last = self.max.min(heights.len().saturating_sub(2));
        let mut found: Vec<(usize, f64)> = (self.min.max(1)..=last)
            .filter(|&l| {
                let h = heights[l];
                h > 0.0 && h > heights[l - 1] && h >= heights[l + 1]
            })
            .map(|l| (l, heights[l]))
            .collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        found
            .into_iter()
            .take(PEAKS)
            .map(|(lag, height)| {
                let period_secs = lag as f64 / self.rate;
                Peak {
                    period_secs,
                    bpm: 60.0 / period_secs,
                    height,
                }
            })
            .collect()
    }
}

fn stats(values: impl Iterator<Item = (f64, f64)>) -> Option<Stats> {
    let held: Vec<(f64, f64)> = values.collect();
    let first = *held.first()?;
    let pick = |better: fn(f64, f64) -> bool| {
        let (t, value) = held
            .iter()
            .copied()
            .fold(first, |a, b| if better(b.1, a.1) { b } else { a });
        Extreme { value, t_secs: t }
    };
    Some(Stats {
        mean: held.iter().map(|(_, v)| v).sum::<f64>() / held.len() as f64,
        min: pick(|a, b| a < b),
        max: pick(|a, b| a > b),
    })
}

fn summary(windows: &[Window]) -> Summary {
    let leads = || {
        windows
            .iter()
            .filter_map(|w| w.peaks.first().map(|p| (w.t_secs, *p)))
    };
    Summary {
        windows_with_peak: leads().count(),
        bpm: stats(leads().map(|(t, p)| (t, p.bpm))),
        height: stats(leads().map(|(t, p)| (t, p.height))),
        drift_bpm: stats(
            windows
                .iter()
                .filter_map(|w| w.drift_bpm.map(|d| (w.t_secs, d))),
        ),
    }
}
