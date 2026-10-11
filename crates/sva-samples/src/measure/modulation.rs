// Concern: how each band group's envelopes and centroid move per sliding window: rate, depth | Non-concern: the band filters, any verdict | IO: (&[f64], rate, start, hop, window, bpm) -> Modulation

use std::f64::consts::{PI, TAU};
use std::ops::Range;

use sva_formula::C64;
use sva_formula::filter::Shape;

use super::bands::{self, Bands, Smoothing};
use super::periodicity::SPLIT_HZ;
use super::sliding::{self, Span, Stats, Unread, stats};
use crate::biquad::{Coeffs, design};
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
                         where they reach past the target. G(f) = |H(f)| L(f), H the \
                         smoothing's response on the energy and L the 50 Hz lowpass's on the \
                         envelope, off their coefficients, is what a modulation at f keeps to \
                         first order, and each envelope peak's depth is divided by G at its \
                         rate, a centroid peak's by L; `passband_gain_at_max_hz` is G at \
                         `max_hz`. The first \
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
                         the sum of c, linear amplitude. peaks: the three local maxima of \
                         depth from `min_hz` to `max_hz` whose bins read deepest, the deepest \
                         after correction first, each placed by inverting the periodic Hann's \
                         exact transform W through its bin and the deeper neighbour, as \
                         Grandke, T. (1983), Interpolation algorithms for discrete Fourier \
                         transforms of weighted signals, IEEE Trans. Instrum. Meas. 32(2) \
                         350-355, does between unpadded bins: the offset u, within q of the \
                         bin, where |W(q - u)| / |W(u)| is the two bins' ratio, q the padded \
                         bin in window bins, that ratio rising with u; its depth the bin's \
                         over |W(u)| / W(0), then over G. Bounds: under the model that in the \
                         window each band's envelope ahead of the smoothing is a carrier's \
                         amplitude c (1 + m cos(2 pi f t + phi)), one m <= 1 in every band, \
                         the carrier's ripple wholly removed by the smoothing, and a centroid \
                         is mean + depth cos(2 pi f t + phi), the two bins read the main lobe \
                         moved by: the modulation's image at -f and the removed mean, at most \
                         r(2 (x - q)) + 2 r(x - q)^2 of its amplitude, x the bin in window \
                         bins, r(y) the transform's envelope over its sidelobes (|W| with \
                         |sin pi y| taken as 1, falling for 1 < y < n/2), the mean also moving \
                         the level by m r(x - q); and for an envelope the square law: the \
                         smoothing h, positive of unit sum, acts on energy, so each envelope \
                         is h * a plus d, 0 <= d = var_h(a) / (sqrt(h * a^2) + h * a) <= c \
                         min(m^2 V / (2 (1 - m)), m sqrt V), V = (1 - |H(f)|^2 + |H(2f) - \
                         H(f)^2|) / 2 at the bin's rate plus q, V rising with f, so the \
                         lowpass (S the sum of its taps' magnitudes) moves a bin by at most \
                         (1 + 3 r(x - q)) S c min(...) and the level by S c min(...). The \
                         ratio widened by those, \
                         inverted, gives `hz_bounds`; the bin over the kernel's most and \
                         least across them and over G's (|H| falling with f, L moving no \
                         faster than its taps' slope) gives `depth_bounds`; m's ceiling is \
                         1, or lower where the square law's sqrt V branch bounds it, and each \
                         round's depth ceiling bounds the next. `period_beats_bounds` follow \
                         from `hz_bounds`. They cover the bins, the interpolation, the \
                         correction and the square law alone, under that model: a tone's \
                         sidebands f either side meet each band's skirt unevenly, so the \
                         bands' envelopes carry a tone's index changed, by up to several \
                         percent near `max_hz` with where the tone falls among the bands, \
                         which is the bands' view and not an error the bounds cover; a second \
                         modulation, partial or component near f is outside the model. They \
                         are null where the window is clipped, where no round bounds the \
                         peak (the perturbations swamping the main lobe), or where the \
                         depth's floor stands over its ceiling, refuting the model. Deep, fast \
                         modulation bounds wide: near `max_hz` the square law's bound reaches \
                         a fifth of m. A strong peak's Hann sidelobes, some \
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

/// `depth` is a modulation index for an envelope, Hz for a centroid. Each bound is MODEL's,
/// `None` where the window leaves or refutes MODEL's lone modulation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub hz: f64,
    pub hz_bounds: Option<(f64, f64)>,
    pub period_beats: Option<f64>,
    pub period_beats_bounds: Option<(f64, f64)>,
    pub depth: f64,
    pub depth_bounds: Option<(f64, f64)>,
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
        size,
        rate,
        sample_rate,
        smoothing: design(Shape::OnePole, SMOOTHING_HZ, 0.0, 0.0, sample_rate),
        lowpass: &lowpass,
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
    Ok(Modulation {
        hop_secs: grid.hop_secs,
        window_secs: grid.window_secs,
        envelope_hz: rate,
        bin_step_hz: step,
        resolution_hz: 2.0 / grid.window_secs,
        min_hz: MIN_HZ.max(2.0 / grid.window_secs),
        max_hz: MAX_HZ,
        bpm,
        passband_gain_at_max_hz: read.gain(MAX_HZ, Kind::Envelope),
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

    /// Over the gain's slope in hz: each tap's cosine turns no faster than its lag.
    fn slope(&self) -> f64 {
        self.taps
            .iter()
            .enumerate()
            .map(|(i, h)| h.abs() * TAU * (i as f64 - HALF_TAPS as f64).abs() / self.rate)
            .sum()
    }

    /// The most the output can swing for an input swinging over a unit range.
    fn spread(&self) -> f64 {
        self.taps.iter().map(|h| h.abs()).sum()
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

struct Read<'a> {
    step: f64,
    /// From the first bin past 0 Hz to one past `MAX_HZ`.
    bins: Range<usize>,
    n: usize,
    bpm: Option<f64>,
    /// A whole window's envelope samples.
    size: usize,
    rate: f64,
    sample_rate: f64,
    smoothing: Coeffs,
    lowpass: &'a Lowpass,
}

/// An envelope's depth is an index under the smoothing's square law; a centroid's is Hz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Envelope,
    Centroid,
}

/// Each round of `Read::bounded` tightens the index's ceiling the last one bounded.
const ROUNDS: usize = 8;

impl Read<'_> {
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
                self.peaks(&depth, lowest, s, Kind::Envelope)
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
                peaks: self.peaks(&depth, lowest, s, Kind::Centroid),
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

    /// `depth` is indexed from `bins.start`: the three local maxima whose bins read deepest,
    /// each located as MODEL states, the deepest first.
    fn peaks(&self, depth: &[f64], lowest: f64, s: &Span, kind: Kind) -> Vec<Peak> {
        let first = self.bins.start;
        let mut found: Vec<usize> = (1..depth.len().saturating_sub(1))
            .filter(|&i| {
                let hz = (first + i) as f64 * self.step;
                (lowest..=MAX_HZ).contains(&hz)
                    && depth[i] > depth[i - 1]
                    && depth[i] >= depth[i + 1]
            })
            .collect();
        found.sort_by(|a, b| depth[*b].total_cmp(&depth[*a]).then(a.cmp(b)));
        let mut peaks: Vec<Peak> = found
            .into_iter()
            .take(PEAKS)
            .map(|i| self.located(depth, i, s.to - s.from, kind))
            .collect();
        peaks.sort_by(|a, b| b.depth.total_cmp(&a.depth).then(a.hz.total_cmp(&b.hz)));
        peaks
    }

    /// The Hann kernel inverted through the deepest bin and its deeper neighbour, and MODEL's
    /// bounds where the window holds its whole span.
    fn located(&self, depth: &[f64], i: usize, size: usize, kind: Kind) -> Peak {
        let q = size as f64 / self.n as f64;
        let x = (self.bins.start + i) as f64 * q;
        let side = match depth[i + 1] >= depth[i - 1] {
            true => 1.0,
            false => -1.0,
        };
        let d0 = depth[i];
        let ratio = depth[(i as f64 + side) as usize] / d0;
        let u = offset(ratio, q, size);
        let hz = (x + side * u) * self.rate / size as f64;
        let bounds = match size == self.size {
            true => self.bounded(d0, ratio, x, side, size, kind),
            false => None,
        };
        let beats = |hz: f64| self.bpm.map(|bpm| bpm / (60.0 * hz));
        Peak {
            hz,
            hz_bounds: bounds.map(|b| b.0),
            period_beats: beats(hz),
            period_beats_bounds: bounds.and_then(|((lo, hi), _)| beats(hi).zip(beats(lo))),
            depth: d0 / kernel(u, size) / self.gain(hz, kind),
            depth_bounds: bounds.map(|b| b.1),
        }
    }

    /// MODEL's (hz, depth) bounds for a peak at natural bin `x` reading `d0`, its neighbour on
    /// `side` reading `ratio` of that; `None` where no round can bound it.
    fn bounded(
        &self,
        d0: f64,
        ratio: f64,
        x: f64,
        side: f64,
        size: usize,
        kind: Kind,
    ) -> Option<((f64, f64), (f64, f64))> {
        let q = size as f64 / self.n as f64;
        let to_hz = |nu: f64| nu * self.rate / size as f64;
        let kappa = reach(x - q, size);
        let lambda = reach(2.0 * (x - q), size) + 2.0 * kappa * kappa;
        let spread = self.lowpass.spread();
        let v = match kind {
            Kind::Envelope => self.square_law(to_hz(x + q)),
            Kind::Centroid => 0.0,
        };
        let law = |m: f64| {
            let tight = match m < 1.0 {
                true => m * m * v / (2.0 * (1.0 - m)),
                false => f64::INFINITY,
            };
            tight.min(m * v.sqrt())
        };
        let square = |m: f64| (1.0 + 3.0 * kappa) * spread * law(m);
        let mut m = match kind {
            Kind::Envelope => {
                let (lo, _) = self.gains((to_hz(x - q), to_hz(x + q)), kind);
                let room = lo * kernel(q, size)
                    - d0 * (kappa + spread * v.sqrt())
                    - spread * lambda
                    - square(1.0);
                match room > 0.0 {
                    true => (d0 / room).min(1.0),
                    false => 1.0,
                }
            }
            Kind::Centroid => f64::INFINITY,
        };
        let mut out = None;
        for _ in 0..ROUNDS {
            let (level, amp) = match kind {
                Kind::Envelope => (m * kappa + spread * law(m), m * spread),
                Kind::Centroid => (0.0, d0 / (kernel(q, size) - lambda)),
            };
            let e = amp * lambda + square(m);
            let floor = d0 * (1.0 - level) - e;
            if !(floor > 0.0 && amp > 0.0 && amp.is_finite()) {
                break;
            }
            let wide = (1.0 + ratio) * e / floor;
            let (ua, ub) = (offset(ratio - wide, q, size), offset(ratio + wide, q, size));
            let near = match ua <= 0.0 && ub >= 0.0 {
                true => 0.0,
                false => ua.abs().min(ub.abs()),
            };
            let far = ua.abs().max(ub.abs());
            let (ha, hb) = (to_hz(x + side * ua), to_hz(x + side * ub));
            let hz = (ha.min(hb), ha.max(hb));
            let g = self.gains(hz, kind);
            let amps = (
                floor / kernel(near, size),
                (d0 * (1.0 + level) + e) / kernel(far, size),
            );
            let depth = (amps.0 / g.1, amps.1 / g.0);
            if depth.0 > m {
                return None;
            }
            out = Some((hz, (depth.0, depth.1.min(m))));
            if kind == Kind::Centroid || depth.1 >= m {
                break;
            }
            m = depth.1;
        }
        out
    }

    /// The smoothing's response on energy at `hz`.
    fn smoothed(&self, hz: f64) -> C64 {
        let w = TAU * hz / self.sample_rate;
        let back = C64::new(w.cos(), -w.sin()).scale(self.smoothing.a1);
        (C64::real(self.smoothing.b0) / (C64::ONE + back)).powi(SMOOTHING.poles as u32)
    }

    /// What a lone modulation at `hz` keeps to first order: the smoothing's gain on energy
    /// and the lowpass's for an envelope, the lowpass's alone for a centroid.
    fn gain(&self, hz: f64, kind: Kind) -> f64 {
        match kind {
            Kind::Envelope => self.smoothed(hz).abs() * self.lowpass.gain(hz),
            Kind::Centroid => self.lowpass.gain(hz),
        }
    }

    /// `gain`'s least and most over `hz`: the smoothing's falls with hz, the lowpass's moves
    /// no faster than its slope from the middle.
    fn gains(&self, hz: (f64, f64), kind: Kind) -> (f64, f64) {
        let (mid, half) = ((hz.0 + hz.1) / 2.0, (hz.1 - hz.0) / 2.0);
        let pass = self.lowpass.gain(mid);
        let moved = self.lowpass.slope() * half;
        let (lo, hi) = (pass - moved, pass + moved);
        match kind {
            Kind::Envelope => (
                self.smoothed(hz.1).abs() * lo,
                self.smoothed(hz.0).abs() * hi,
            ),
            Kind::Centroid => (lo, hi),
        }
    }

    /// V at `hz`: (1 - |H(f)|^2 + |H(2f) - H(f)^2|) / 2, H the smoothing on energy.
    fn square_law(&self, hz: f64) -> f64 {
        let (one, two) = (self.smoothed(hz), self.smoothed(2.0 * hz));
        (1.0 - one.norm_sqr() + (two - one * one).abs()) / 2.0
    }
}

/// The periodic Hann over `n` samples, its transform as three Dirichlet kernels: at `x` of
/// its bins, sin(pi x) times this.
fn dirichlet(x: f64, n: usize) -> C64 {
    let n = n as f64;
    let turn = C64::new((PI / n).cos(), (PI / n).sin());
    let s = |y: f64| (PI * y / n).sin();
    C64::real(0.5 / s(x)) - turn.conj().over(4.0 * s(x - 1.0)) - turn.over(4.0 * s(x + 1.0))
}

/// |W(x)| / W(0), W the periodic Hann's transform over `n` samples at `x` of its bins: exact.
fn kernel(x: f64, n: usize) -> f64 {
    match x.abs() {
        0.0 => 1.0,
        1.0 => 0.5,
        x => 2.0 / n as f64 * (PI * x).sin().abs() * dirichlet(x, n).abs(),
    }
}

/// `kernel` with |sin(pi x)| taken as 1: over it, and falling, for 1 < x < n / 2.
fn reach(x: f64, n: usize) -> f64 {
    2.0 / n as f64 * dirichlet(x, n).abs()
}

/// The u in [-q, q] where kernel(q - u) / kernel(u) is `ratio`, by bisection; that ratio rises
/// with u, so a ratio past either end gives that end.
fn offset(ratio: f64, q: f64, n: usize) -> f64 {
    let (mut lo, mut hi) = (-q, q);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        match kernel(q - mid, n) / kernel(mid, n) < ratio {
            true => lo = mid,
            false => hi = mid,
        }
    }
    (lo + hi) / 2.0
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The closed form against the transform summed sample by sample; `reach` over it and
    /// falling on (1, n/2); V rising with f to `MAX_HZ`: what MODEL's bounds lean on.
    #[test]
    fn the_kernel_its_reach_and_the_square_law_hold_as_model_states() {
        for n in [250, 400, 401] {
            let w = hann_periodic(n);
            let total: f64 = w.iter().sum();
            for x in [0.0, 0.01, 0.39, 0.5, 1.0, 1.7, 2.3, 7.5, 61.2] {
                let summed = w.iter().enumerate().fold(C64::ZERO, |acc, (k, h)| {
                    let turn = -TAU * x * k as f64 / n as f64;
                    acc + C64::new(turn.cos(), turn.sin()).scale(*h)
                });
                assert!(
                    (kernel(x, n) - summed.abs() / total).abs() < 1e-12,
                    "{n} {x}"
                );
            }
            let ys: Vec<f64> = (1..(n * 5)).map(|k| 1.0 + k as f64 / 10.0).collect();
            for pair in ys.windows(2).filter(|p| p[1] < n as f64 / 2.0) {
                assert!(reach(pair[1], n) < reach(pair[0], n), "{n} {pair:?}");
                assert!(kernel(pair[1], n) <= reach(pair[1], n) * (1.0 + 1e-12));
            }
        }
        let lowpass = Lowpass::new(2000.0);
        for sample_rate in [8_192.0, 16_000.0, 44_100.0, 48_000.0, 96_000.0] {
            let read = Read {
                step: 1.0,
                bins: 1..2,
                n: 2,
                bpm: None,
                size: 1,
                rate: 100.0,
                sample_rate,
                smoothing: design(Shape::OnePole, SMOOTHING_HZ, 0.0, 0.0, sample_rate),
                lowpass: &lowpass,
            };
            let v: Vec<f64> = (1..=2000)
                .map(|k| read.square_law(k as f64 * 0.01))
                .collect();
            assert!(v.windows(2).all(|p| p[1] > p[0]), "{sample_rate}");
        }
    }
}
