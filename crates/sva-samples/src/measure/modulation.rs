// Concern: how each band group's envelopes and centroid move per sliding window: rate, depth | Non-concern: the band filters, any verdict | IO: (&[f64], rate, start, hop, window, bpm) -> Modulation

use std::f64::consts::{PI, TAU};
use std::ops::Range;

use sva_formula::filter::Shape;

use super::bands::{self, Bands, Smoothing};
use super::periodicity::SPLIT_HZ;
use super::sliding::{self, Span, Stats, Unread, stats};
use crate::biquad::{design, magnitude_db};
use crate::fft::fft;
use crate::stft::hann_periodic;

/// `low` under the first, `mid` under the second, `high` from it.
pub const SPLITS_HZ: [f64; 2] = [SPLIT_HZ, 2000.0];
pub const MIN_HZ: f64 = 0.5;
pub const MAX_HZ: f64 = 20.0;
pub const PEAKS: usize = 3;
/// Each band's energy through four one-poles at 200 Hz before `bands` decimates it to 2 kHz:
/// a tone's ripple at twice its frequency, near a multiple of 2 kHz, would alias onto a slow
/// modulation, and falls some 78 dB by 1.92 kHz, while a 20 Hz modulation keeps 98%.
pub const SMOOTHING: Smoothing = Smoothing {
    hz: Some(SMOOTHING_HZ),
    poles: 4,
};
const SMOOTHING_HZ: f64 = 200.0;
/// The rate the envelopes are read at once the lowpass below has cleared the band.
const ENVELOPE_HZ: f64 = 100.0;
const LOWPASS_HZ: f64 = 50.0;
/// Half the lowpass's taps less one: 257 taps at 2 kHz.
const HALF_TAPS: usize = 128;

pub const MODEL: &str = "modulation spectrum: the spectrum of each band's amplitude envelope \
                         over a sliding window, the representation of Atlas, L. and Shamma, S. \
                         A. (2003), Joint acoustic and modulation frequency, EURASIP J. Appl. \
                         Signal Process. 2003(7) 668-675, read over the 0.5-20 Hz modulation \
                         range the envelope filterbank of Dau, T., Kollmeier, B. and \
                         Kohlrausch, A. (1997), Modeling auditory processing of amplitude \
                         modulation I, J. Acoust. Soc. Am. 102(5) 2892-2905, models. Envelopes: \
                         the RMS of each ERB band of `bands` (Glasberg and Moore's filter \
                         widths), its energy smoothed by `smoothing.poles` one-pole lowpasses at \
                         `smoothing.hz` in place of the one at the band's own width, decimated \
                         to 2 kHz, then by a 257-tap Blackman-windowed sinc lowpass at 50 Hz to \
                         `envelope_hz` (Harris, F. J. (1978), On the use of windows for \
                         harmonic analysis with the discrete Fourier transform, Proc. IEEE \
                         66(1) 51-83: stopband some 74 dB down, from about 71 Hz, so what \
                         would fold into 0-20 Hz is that far down), the taps renormalized \
                         where they reach past the target. `passband_gain_at_max_hz` is the \
                         two lowpasses' gain on the energy at `max_hz`, computed off their \
                         coefficients: a modulation that fast reads that much shallower, a \
                         slower one less so, and no depth is corrected for it. The first \
                         component alone. Groups: a band is `low` where its centre is under \
                         `splits_hz[0]`, `mid` under `splits_hz[1]` and `high` from it; the \
                         filters' skirts overlap, so a tone leaks attenuated into the \
                         neighbouring group, and its modulation with it. Each window is \
                         `window_secs` of envelope centred on t, t stepping by `hop_secs` \
                         from the target's start as every framed reading steps, clipped to \
                         the target at its ends, `from`/`to` its span; a periodic Hann window \
                         w over it, each band's Hann-weighted mean c = sum w e / sum w \
                         subtracted (so the mean leaves no lobe), zero-padded to `bin_step_hz` \
                         and transformed by FFT. depth(f) = 2 sum over the group's bands of \
                         |W(f)| over (sum w times sum of c): the modulation index m of an \
                         envelope c (1 + m cos 2 pi f t), read as the c-weighted mean of the \
                         bands' indices, magnitudes summed so bands moving out of phase, as \
                         under a filter sweep, do not cancel; an envelope near silent \
                         between swells reads past 1, up to 2 for a pulse train. `level` is \
                         the sum of c, linear amplitude. peaks: local maxima of depth from `min_hz` to `max_hz`, \
                         the highest three, highest first, each at the quadratic through the \
                         log depth of its bin and the two beside it (Smith, J. O. and Serra, \
                         X. (1987), PARSHL, Proc. ICMC); a strong peak's Hann sidelobes, some \
                         31 dB under it and 1.5 / window beside it, read as peaks of their \
                         own; a modulation that is no sinusoid reads at its rate and its \
                         harmonics; two modulations closer than \
                         `resolution_hz`, the Hann main lobe's half-width 2 / window, or one \
                         slower than that, interfere and are not resolved, and a window \
                         clipped at the target's ends reads from 2 / (to - from) Hz. \
                         period_beats, where `bpm` is given: bpm / (60 hz), so 0.5 is an \
                         eighth-note wobble. centroid: per instant the group's spectral \
                         centroid, sum of centre e over sum of e across its bands at 2 kHz, \
                         lowpassed and decimated as the envelopes are; `mean_hz` its \
                         Hann-weighted mean, and its peaks read as the envelopes' are, their \
                         `depth_hz` the amplitude in Hz of a centroid swinging as mean + depth \
                         cos 2 pi f t, which a filter sweep moves and tremolo does not; null \
                         where any instant in the window has no energy in the group. Beating \
                         between partials inside one band reads as modulation at their \
                         difference, as it is heard; a modulation under `min_hz` (a pad \
                         evolving over seconds) reads in `level` and `centroid.mean_hz` from \
                         window to window, not in the peaks. No threshold: a static sound \
                         reads peaks of small depth, never none. summary: the leading peak's \
                         hz and depth over the windows with a peak, the centroid's mean_hz \
                         over the windows with one, the leading centroid peak's depth_hz over \
                         the windows with one, each its mean, min and max with the t each \
                         stands at. Exact path: none; a closed form is read \
                         off its rendered samples, because an envelope's band-limited \
                         spectrum over a window is stated by no closed form";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Low,
    Mid,
    High,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Low, Group::Mid, Group::High];

    pub fn key(self) -> &'static str {
        match self {
            Group::Low => "low",
            Group::Mid => "mid",
            Group::High => "high",
        }
    }
}

/// `depth` is a modulation index for an envelope, Hz for a centroid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub hz: f64,
    pub period_beats: Option<f64>,
    pub depth: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Centroid {
    pub mean_hz: f64,
    /// Highest first.
    pub peaks: Vec<Peak>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub t_secs: f64,
    pub from_secs: f64,
    pub to_secs: f64,
    pub level: f64,
    /// Highest first.
    pub peaks: Vec<Peak>,
    pub centroid: Option<Centroid>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub windows_with_peak: usize,
    pub hz: Option<Stats>,
    pub depth: Option<Stats>,
    pub centroid_hz: Option<Stats>,
    pub centroid_depth_hz: Option<Stats>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub group: Group,
    pub centres_hz: Vec<f64>,
    pub windows: Vec<Window>,
    pub summary: Summary,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Modulation {
    pub hop_secs: f64,
    pub window_secs: f64,
    pub envelope_hz: f64,
    pub bin_step_hz: f64,
    pub resolution_hz: f64,
    pub min_hz: f64,
    pub max_hz: f64,
    pub bpm: Option<f64>,
    pub passband_gain_at_max_hz: f64,
    pub tracks: Vec<Track>,
}

pub fn measure(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    hop_secs: f64,
    window_secs: f64,
    bpm: Option<f64>,
) -> Result<Modulation, Unread> {
    sliding::fits(samples.len(), sample_rate, window_secs, 1.0 / MIN_HZ)?;
    let split = bands::smoothed(samples, sample_rate, start_secs, SMOOTHING);
    let lowpass = Lowpass::new(split.rate_hz);
    let groups = grouped(&split, &lowpass);
    let rate = split.rate_hz / lowpass.step as f64;
    let len = groups.first().map_or(0, |g| g.centroid.len());
    let grid = sliding::spans(
        samples.len(),
        sample_rate,
        start_secs,
        hop_secs,
        rate,
        window_secs,
        len,
    );
    let size = (grid.window_secs * rate).round() as usize;
    let n = (2 * size).next_power_of_two();
    let step = rate / n as f64;
    let read = Read {
        step,
        bins: 1..((MAX_HZ / step).ceil() as usize + 2).min(n / 2),
        n,
        bpm,
    };
    let tracks = Group::ALL
        .into_iter()
        .zip(groups)
        .map(|(group, held)| {
            let windows: Vec<Window> = grid.spans.iter().map(|s| read.window(&held, s)).collect();
            Track {
                group,
                centres_hz: held.centres_hz,
                summary: summary(&windows),
                windows,
            }
        })
        .collect();
    let smoothing = design(Shape::OnePole, SMOOTHING_HZ, 0.0, 0.0, sample_rate);
    let smoothing_db = SMOOTHING.poles as f64 * magnitude_db(&smoothing, MAX_HZ, sample_rate);
    Ok(Modulation {
        hop_secs: grid.hop_secs,
        window_secs: grid.window_secs,
        envelope_hz: rate,
        bin_step_hz: step,
        resolution_hz: 2.0 / grid.window_secs,
        min_hz: MIN_HZ.max(2.0 / grid.window_secs),
        max_hz: MAX_HZ,
        bpm,
        passband_gain_at_max_hz: 10f64.powf(smoothing_db / 20.0) * lowpass.gain(MAX_HZ),
        tracks,
    })
}

/// A Blackman-windowed sinc, unit gain at 0 Hz, taken every `step` samples.
struct Lowpass {
    taps: Vec<f64>,
    step: usize,
    rate: f64,
}

impl Lowpass {
    fn new(rate: f64) -> Lowpass {
        let cut = LOWPASS_HZ / rate;
        let half = HALF_TAPS as f64;
        let raw: Vec<f64> = (0..=2 * HALF_TAPS)
            .map(|i| {
                let j = i as f64 - half;
                let sinc = match j == 0.0 {
                    true => 2.0 * cut,
                    false => (TAU * cut * j).sin() / (PI * j),
                };
                let x = PI * j / half;
                sinc * (0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos())
            })
            .collect();
        let sum: f64 = raw.iter().sum();
        Lowpass {
            taps: raw.into_iter().map(|h| h / sum).collect(),
            step: (rate / ENVELOPE_HZ).round().max(1.0) as usize,
            rate,
        }
    }

    /// Symmetric, so its response is real.
    fn gain(&self, hz: f64) -> f64 {
        self.taps
            .iter()
            .enumerate()
            .map(|(i, h)| h * (TAU * hz * (i as f64 - HALF_TAPS as f64) / self.rate).cos())
            .sum()
    }

    /// Centred on each kept sample, the taps past either end left out and the rest
    /// renormalized, so a constant stays that constant to the last sample. NaN in reach
    /// stays NaN.
    fn apply(&self, x: &[f64]) -> Vec<f64> {
        (0..x.len())
            .step_by(self.step)
            .map(|m| {
                let lo = m.saturating_sub(HALF_TAPS);
                let hi = (m + HALF_TAPS + 1).min(x.len());
                let taps = &self.taps[lo + HALF_TAPS - m..hi + HALF_TAPS - m];
                let sum: f64 = taps.iter().sum();
                taps.iter().zip(&x[lo..hi]).map(|(h, v)| h * v).sum::<f64>() / sum
            })
            .collect()
    }
}

/// One group's decimated band envelopes and centroid.
struct Held {
    centres_hz: Vec<f64>,
    envelopes: Vec<Vec<f64>>,
    /// NaN where the group holds no energy.
    centroid: Vec<f64>,
}

fn grouped(split: &Bands, lowpass: &Lowpass) -> Vec<Held> {
    let tracks = &split.bands;
    let edge = |hz: f64| {
        tracks
            .iter()
            .position(|b| b.centre_hz >= hz)
            .unwrap_or(tracks.len())
    };
    let (low, mid) = (edge(SPLITS_HZ[0]), edge(SPLITS_HZ[1]));
    let len = tracks.first().map_or(0, |b| b.rms.len());
    [0..low, low..mid, mid..tracks.len()]
        .into_iter()
        .map(|range: Range<usize>| {
            let held = &tracks[range];
            let centroid: Vec<f64> = (0..len)
                .map(|i| {
                    let (weighted, sum) = held.iter().fold((0.0, 0.0), |(w, s), b| {
                        (w + b.centre_hz * b.rms[i], s + b.rms[i])
                    });
                    match sum > 0.0 {
                        true => weighted / sum,
                        false => f64::NAN,
                    }
                })
                .collect();
            Held {
                centres_hz: held.iter().map(|b| b.centre_hz).collect(),
                envelopes: held.iter().map(|b| lowpass.apply(&b.rms)).collect(),
                centroid: lowpass.apply(&centroid),
            }
        })
        .collect()
}

struct Read {
    step: f64,
    /// From the first bin past 0 Hz to one past `MAX_HZ`.
    bins: Range<usize>,
    n: usize,
    bpm: Option<f64>,
}

impl Read {
    fn window(&self, held: &Held, s: &Span) -> Window {
        let w = hann_periodic(s.to - s.from);
        let total: f64 = w.iter().sum();
        let lowest = MIN_HZ.max(2.0 / (s.to_secs - s.from_secs));
        let segments: Vec<&[f64]> = held.envelopes.iter().map(|e| &e[s.from..s.to]).collect();
        let means: Vec<f64> = segments.iter().map(|e| weighted_mean(&w, e)).collect();
        let level: f64 = means.iter().sum();
        let peaks = match level > 0.0 {
            true => {
                let mut depth = vec![0.0; self.bins.len()];
                for spectrum in self.magnitudes(&w, &segments, &means) {
                    for (d, m) in depth.iter_mut().zip(spectrum) {
                        *d += 2.0 * m / (total * level);
                    }
                }
                self.peaks(&depth, lowest)
            }
            false => Vec::new(),
        };
        let track = &held.centroid[s.from..s.to];
        let centroid = (!track.iter().any(|c| c.is_nan())).then(|| {
            let mean_hz = weighted_mean(&w, track);
            let spectrum = self.magnitudes(&w, &[track], &[mean_hz]).remove(0);
            let depth: Vec<f64> = spectrum.into_iter().map(|m| 2.0 * m / total).collect();
            Centroid {
                mean_hz,
                peaks: self.peaks(&depth, lowest),
            }
        });
        Window {
            t_secs: s.t_secs,
            from_secs: s.from_secs,
            to_secs: s.to_secs,
            level,
            peaks,
            centroid,
        }
    }

    /// |DFT| of each windowed, mean-removed segment at `bins`; two segments share one complex
    /// transform as its real and imaginary parts.
    fn magnitudes(&self, w: &[f64], segments: &[&[f64]], means: &[f64]) -> Vec<Vec<f64>> {
        let fill = |out: &mut [f64], at: usize| {
            for ((o, x), h) in out.iter_mut().zip(segments[at]).zip(w) {
                *o = h * (x - means[at]);
            }
        };
        let mut out = Vec::with_capacity(segments.len());
        for pair in (0..segments.len()).step_by(2) {
            let (mut re, mut im) = (vec![0.0; self.n], vec![0.0; self.n]);
            fill(&mut re, pair);
            let both = pair + 1 < segments.len();
            if both {
                fill(&mut im, pair + 1);
            }
            fft(&mut re, &mut im);
            let half = |sign: f64| -> Vec<f64> {
                self.bins
                    .clone()
                    .map(|k| {
                        let (r, i) = (re[self.n - k], -im[self.n - k]);
                        (re[k] + sign * r).hypot(im[k] + sign * i) / 2.0
                    })
                    .collect()
            };
            out.push(half(1.0));
            if both {
                out.push(half(-1.0));
            }
        }
        out
    }

    /// `depth` is indexed from `bins.start`; each peak is placed by the quadratic through
    /// the log of its bin and its two neighbours.
    fn peaks(&self, depth: &[f64], lowest: f64) -> Vec<Peak> {
        let first = self.bins.start;
        let mut found: Vec<(f64, f64)> = (1..depth.len().saturating_sub(1))
            .filter(|&i| {
                let hz = (first + i) as f64 * self.step;
                (lowest..=MAX_HZ).contains(&hz)
                    && depth[i] > depth[i - 1]
                    && depth[i] >= depth[i + 1]
            })
            .map(|i| {
                let (a, b, c) = (depth[i - 1], depth[i], depth[i + 1]);
                let (shift, top) = match a > 0.0 && c > 0.0 {
                    true => {
                        let (a, b, c) = (a.ln(), b.ln(), c.ln());
                        let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
                        (shift, (b - 0.25 * (a - c) * shift).exp())
                    }
                    false => (0.0, b),
                };
                ((first + i) as f64 + shift, top)
            })
            .collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.total_cmp(&b.0)));
        found
            .into_iter()
            .take(PEAKS)
            .map(|(bin, depth)| {
                let hz = bin * self.step;
                Peak {
                    hz,
                    period_beats: self.bpm.map(|bpm| bpm / (60.0 * hz)),
                    depth,
                }
            })
            .collect()
    }
}

fn weighted_mean(w: &[f64], x: &[f64]) -> f64 {
    w.iter().zip(x).map(|(h, v)| h * v).sum::<f64>() / w.iter().sum::<f64>()
}

fn summary(windows: &[Window]) -> Summary {
    let leads = || {
        windows
            .iter()
            .filter_map(|w| w.peaks.first().map(|p| (w.t_secs, *p)))
    };
    let centroids = || {
        windows
            .iter()
            .filter_map(|w| w.centroid.as_ref().map(|c| (w.t_secs, c)))
    };
    Summary {
        windows_with_peak: leads().count(),
        hz: stats(leads().map(|(t, p)| (t, p.hz))),
        depth: stats(leads().map(|(t, p)| (t, p.depth))),
        centroid_hz: stats(centroids().map(|(t, c)| (t, c.mean_hz))),
        centroid_depth_hz: stats(
            centroids().filter_map(|(t, c)| c.peaks.first().map(|p| (t, p.depth))),
        ),
    }
}
