// Concern: how strongly onset strength repeats per sliding window: broadband, low, rest | Non-concern: the band filters (bands.rs), any verdict | IO: (&[f64], rate, start, hop, window) -> Periodicity

use super::bands::{self, Bands, Smoothing};
use super::onsets::spectral_flux;
use super::sliding::{self, Stats, Unread, stats};
use crate::fft::{fft, ifft};
use crate::stft::hann_periodic;

/// A band belongs to `low` where its centre is under this; kick and sub sit there.
pub const SPLIT_HZ: f64 = 150.0;
/// 600 bpm, a sixteenth at 150 bpm.
pub const MIN_PERIOD_SECS: f64 = 0.1;
/// 30 bpm.
pub const MAX_PERIOD_SECS: f64 = 2.0;
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
                         spans are not on one scale. place(tau): the same y under a periodic \
                         Hann taper w, sum of y[n] w[n] y[n + tau] w[n + tau] over sum of w[n] \
                         w[n + tau], the taper's own autocorrelation divided out: the window's \
                         ends no longer cut an onset abruptly, which in the plain sum skews a \
                         peak by up to a quarter lag, and the bias toward short lags is gone \
                         (Harris, F. J. (1978), On the use of windows for harmonic analysis \
                         with the discrete Fourier transform, Proc. IEEE 66(1) 51-83). peaks: \
                         lags L from `min_period_secs` to `max_period_secs` whose height is \
                         positive and whose place is over the lag before's and no lower than \
                         the lag after's, the highest three by height, highest first, each \
                         with the plain curve's height at L. period: L + d lags of \
                         `lag_step_secs`, d the vertex of the parabola through place at L - 1, \
                         L and L + 1 (Smith, J. O. and Serra, X. (1987), PARSHL, Proc. ICMC), \
                         |d| <= 1/2; bpm = 60 / period. Bounds: under the model that place is \
                         symmetric about the true period P and falls strictly away from it, L \
                         reading at least its neighbours puts P within half a lag of L, and \
                         the higher neighbour names the half: P lies in [L, L + 1/2] where \
                         place at L + 1 is over place at L - 1, in [L - 1/2, L] otherwise; d \
                         lies in that half too, so P is within r of the period, r the farther \
                         end of that half from it, r <= 1/2 lag. `period_bounds_secs` is \
                         period -+ r and `bpm_bounds` 60 over those. They cover the lag grid \
                         and the interpolation alone, under that model, which no window can \
                         confirm: an envelope not periodic across the window (a tempo moving, \
                         onsets jittered) is outside it, and its place need not peak at any \
                         one period. Both are null where the window refutes the model, place \
                         not falling strictly from L - 1 to L - 2 and from L + 1 to L + 2, or \
                         leaves the stated span, clipped at the target's ends. No tempo prior \
                         (Ellis weights by a log-Gaussian) and no octave choice: a beat, its \
                         half and its double may each lead, and each is reported; no inter-onset \
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
    /// Where MODEL's symmetric peak puts the true period; `None` where the window refutes or
    /// leaves that model.
    pub period_bounds_secs: Option<(f64, f64)>,
    pub bpm: f64,
    /// 60 over `period_bounds_secs`.
    pub bpm_bounds: Option<(f64, f64)>,
    /// The plain curve's height at the peak's lag.
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

pub fn measure(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    hop_secs: f64,
    window_secs: f64,
    curve: bool,
) -> Result<Periodicity, Unread> {
    sliding::fits(samples.len(), sample_rate, window_secs, MAX_PERIOD_SECS)?;
    let split = bands::smoothed(samples, sample_rate, start_secs, SMOOTHING);
    let rate = split.rate_hz;
    let lags = Lags {
        min: (MIN_PERIOD_SECS * rate).ceil() as usize,
        max: (MAX_PERIOD_SECS * rate).floor() as usize,
        rate,
    };
    let len = split.bands.first().map_or(0, |b| b.rms.len());
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
    let tracks = Group::ALL
        .into_iter()
        .zip(envelopes(&split))
        .map(|(group, (centres_hz, envelope))| {
            let mut windows: Vec<Window> = grid
                .spans
                .iter()
                .map(|s| {
                    let (heights, place) = heights(&envelope[s.from..s.to], lags.max + 1);
                    Window {
                        t_secs: s.t_secs,
                        from_secs: s.from_secs,
                        to_secs: s.to_secs,
                        peaks: lags.peaks(&heights, &place, s.to - s.from == size),
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
        hop_secs: grid.hop_secs,
        window_secs: grid.window_secs,
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

/// At lags `0..=top` the segment holds: the mean-removed segment's autocorrelation over its
/// value at lag 0, and MODEL's place, the curve a peak is located on.
fn heights(segment: &[f64], top: usize) -> (Vec<f64>, Vec<f64>) {
    let top = top.min(segment.len().saturating_sub(1));
    let mean = segment.iter().sum::<f64>() / segment.len() as f64;
    let y: Vec<f64> = segment.iter().map(|x| x - mean).collect();
    let taper = hann_periodic(segment.len());
    let tapered: Vec<f64> = y.iter().zip(&taper).map(|(y, w)| y * w).collect();
    let plain = correlated(&y, top);
    let (curve, own) = (correlated(&tapered, top), correlated(&taper, top));
    (
        plain.iter().map(|r| r / plain[0]).collect(),
        curve.iter().zip(&own).map(|(c, w)| c / w).collect(),
    )
}

/// sum over n of x[n] x[n + tau] at lags `0..=top`, zero-padded past `len + top` so no lag wraps.
fn correlated(x: &[f64], top: usize) -> Vec<f64> {
    let n = (x.len() + top + 1).next_power_of_two();
    let mut re = vec![0.0; n];
    re[..x.len()].copy_from_slice(x);
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    for (r, i) in re.iter_mut().zip(im.iter_mut()) {
        *r = *r * *r + *i * *i;
        *i = 0.0;
    }
    ifft(&mut re, &mut im);
    re.truncate(top + 1);
    re
}

struct Lags {
    min: usize,
    max: usize,
    rate: f64,
}

impl Lags {
    /// Local maxima of `place`, ranked by `heights`, each located and bounded as MODEL states;
    /// `whole` where the window holds its full span.
    fn peaks(&self, heights: &[f64], place: &[f64], whole: bool) -> Vec<Peak> {
        let last = self.max.min(heights.len().saturating_sub(2));
        let mut found: Vec<(usize, f64)> = (self.min.max(1)..=last)
            .filter(|&l| heights[l] > 0.0 && place[l] > place[l - 1] && place[l] >= place[l + 1])
            .map(|l| (l, heights[l]))
            .collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        found
            .into_iter()
            .take(PEAKS)
            .map(|(lag, height)| {
                let (before, at, after) = (place[lag - 1], place[lag], place[lag + 1]);
                let shift = 0.5 * (before - after) / (before - 2.0 * at + after);
                let half = match after > before {
                    true => (0.0, 0.5),
                    false => (-0.5, 0.0),
                };
                let reach = (shift - half.0).max(half.1 - shift);
                let secs = |l: f64| (lag as f64 + l) / self.rate;
                let period_secs = secs(shift);
                let falls = place[lag - 2] < before
                    && place.get(lag + 2).is_some_and(|&beyond| beyond < after);
                let bounds = (whole && falls).then(|| (secs(shift - reach), secs(shift + reach)));
                Peak {
                    period_secs,
                    period_bounds_secs: bounds,
                    bpm: 60.0 / period_secs,
                    bpm_bounds: bounds.map(|(lo, hi)| (60.0 / hi, 60.0 / lo)),
                    height,
                }
            })
            .collect()
    }
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
