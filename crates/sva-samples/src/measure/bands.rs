// Concern: traces one envelope per ERB band, beside that band's own impulse floor | Non-concern: the discrete spectrum (spectrum.rs), what a band means to a listener | IO: (&[f64], sample rate) -> bands

use sva_formula::filter::Shape;

use crate::biquad::{Coeffs, State, design};

/// Glasberg and Moore's auditory filter width in Hz.
pub fn erb_hz(hz: f64) -> f64 {
    24.7 * (4.37 * hz / 1000.0 + 1.0)
}

/// One Cam is one filter width along the cochlea.
pub fn cam(hz: f64) -> f64 {
    21.4 * (1.0 + 4.37 * hz / 1000.0).log10()
}

pub fn hz_at_cam(cam: f64) -> f64 {
    (10f64.powf(cam / 21.4) - 1.0) * 1000.0 / 4.37
}

/// 1 to 40 Cam is 51 Hz to 16.4 kHz.
pub const FIRST_CAM: f64 = 1.0;
pub const BAND_COUNT: usize = 40;

/// Under the 2-3 ms gap-detection limit, a hundredth of the samples it came from.
pub const DECIMATED_HZ: f64 = 2000.0;

/// Sixteen time constants is past every band's peak.
fn floor_secs(erb: f64) -> f64 {
    (16.0 / erb).clamp(0.02, 2.0)
}

/// A filter cannot report an onset faster than its own, so a caller subtracts this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandFloor {
    pub peak: f64,
    pub time_to_peak_secs: f64,
    pub rise_10_90_secs: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BandTrack {
    pub centre_hz: f64,
    pub cam: f64,
    pub erb_hz: f64,
    pub q: f64,
    pub peak: f64,
    pub time_to_peak_secs: Option<f64>,
    pub rise_10_90_secs: Option<f64>,
    pub floor: BandFloor,
    pub rms: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bands {
    pub rate_hz: f64,
    pub start_secs: f64,
    pub bands: Vec<BandTrack>,
}

/// Squaring rectifies; the root at decimation makes the track an RMS.
struct Chain {
    pass: Coeffs,
    smooth: Coeffs,
    a: State,
    b: State,
    energy: State,
}

impl Chain {
    fn new(centre_hz: f64, sr: f64) -> Chain {
        let erb = erb_hz(centre_hz);
        Chain {
            pass: design(Shape::Bandpass, centre_hz, centre_hz / erb, 0.0, sr),
            smooth: design(Shape::OnePole, erb.min(DECIMATED_HZ / 4.0), 0.0, 0.0, sr),
            a: State::default(),
            b: State::default(),
            energy: State::default(),
        }
    }

    #[inline]
    fn step(&mut self, x: f64) -> f64 {
        let banded = self.b.step(&self.pass, self.a.step(&self.pass, x));
        self.energy.step(&self.smooth, banded * banded)
    }
}

fn trace(centre_hz: f64, sr: f64, hop: usize, n: usize, at: impl Fn(usize) -> f64) -> Vec<f64> {
    let mut chain = Chain::new(centre_hz, sr);
    let mut out = Vec::with_capacity(n / hop + 1);
    for i in 0..n {
        let energy = chain.step(at(i));
        if i % hop == 0 {
            out.push(energy.max(0.0).sqrt());
        }
    }
    out
}

/// Interpolated between the frames straddling it.
fn crossing(track: &[f64], until: usize, level: f64, rate: f64) -> Option<f64> {
    let hit = track[..=until].iter().position(|&v| v >= level)?;
    if hit == 0 {
        return Some(0.0);
    }
    let (lo, hi) = (track[hit - 1], track[hit]);
    let frac = match hi > lo {
        true => (level - lo) / (hi - lo),
        false => 0.0,
    };
    Some((hit as f64 - 1.0 + frac) / rate)
}

fn stats(track: &[f64], rate: f64) -> (f64, Option<f64>, Option<f64>) {
    let peak = track.iter().copied().fold(0.0, f64::max);
    if peak <= 0.0 {
        return (peak, None, None);
    }
    let at = track
        .iter()
        .position(|&v| v == peak)
        .expect("the peak is one of the frames");
    let t10 = crossing(track, at, 0.1 * peak, rate);
    let t90 = crossing(track, at, 0.9 * peak, rate);
    let rise = t10.zip(t90).map(|(a, b)| (b - a).max(0.0));
    (peak, Some(at as f64 / rate), rise)
}

fn floor_of(centre_hz: f64, sr: f64, hop: usize, rate: f64) -> BandFloor {
    let n = (floor_secs(erb_hz(centre_hz)) * sr).round().max(4.0) as usize;
    let track = trace(centre_hz, sr, hop, n, |i| f64::from(u8::from(i == 0)));
    let (peak, at, rise) = stats(&track, rate);
    BandFloor {
        peak,
        time_to_peak_secs: at.unwrap_or(0.0),
        rise_10_90_secs: rise,
    }
}

/// A centre the design cannot place is left out, never clamped onto the last it could.
fn centres(sr: f64) -> Vec<f64> {
    (0..BAND_COUNT)
        .map(|k| hz_at_cam(FIRST_CAM + k as f64))
        .filter(|hz| *hz < 0.45 * sr)
        .collect()
}

pub fn analyze(samples: &[f64], sr: f64, start_secs: f64) -> Bands {
    let hop = (sr / DECIMATED_HZ).round().max(1.0) as usize;
    let rate = sr / hop as f64;
    Bands {
        rate_hz: rate,
        start_secs,
        bands: centres(sr)
            .into_iter()
            .map(|centre_hz| {
                let track = trace(centre_hz, sr, hop, samples.len(), |i| samples[i]);
                let (peak, at, rise) = stats(&track, rate);
                let erb = erb_hz(centre_hz);
                BandTrack {
                    centre_hz,
                    cam: cam(centre_hz),
                    erb_hz: erb,
                    q: centre_hz / erb,
                    peak,
                    time_to_peak_secs: at,
                    rise_10_90_secs: rise,
                    floor: floor_of(centre_hz, sr, hop, rate),
                    rms: track,
                }
            })
            .collect(),
    }
}
