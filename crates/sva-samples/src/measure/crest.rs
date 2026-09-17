// Concern: peak minus RMS in each third-octave band, and the spread across them | Non-concern: the discrete spectrum (spectrum.rs), the ERB envelope bank (bands.rs) | IO: (&[f64], sample rate) -> Crest

use sva_formula::filter::Shape;

use crate::biquad::{State, design};
use crate::measure::envelope::rms;
use crate::measure::spectrum::{db, third_octave_edges};

/// Below this a band holds leakage, which would set the spread.
const COUNTED_UNDER_DB: f64 = 60.0;

/// Five time constants of overshoot, and `sqrt(sqrt(2)-1)` is what a second identical
/// stage leaves of one stage's -3 dB width.
const SETTLE_TAUS: f64 = 5.0;
const CASCADE_Q: f64 = 1.55;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandCrest {
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub centre_hz: f64,
    pub peak: f64,
    pub rms: f64,
    pub crest_db: f64,
    pub counted: bool,
}

/// The spread, not the level: the worst-rated master had the most low-band range.
#[derive(Clone, Debug, PartialEq)]
pub struct Crest {
    pub broadband_crest_db: f64,
    pub spread_db: Option<f64>,
    pub widest_band_hz: Option<f64>,
    pub tightest_band_hz: Option<f64>,
    pub counted_under_db: f64,
    pub bands: Vec<BandCrest>,
}

pub fn analyze(samples: &[f64], sample_rate: f64) -> Crest {
    let mut bands: Vec<BandCrest> = third_octave_edges(sample_rate)
        .into_iter()
        .map(|(lo, hi)| band(samples, sample_rate, lo, hi))
        .collect();

    let loudest = bands.iter().map(|b| b.rms).fold(0.0, f64::max);
    let floor = loudest * 10f64.powf(-COUNTED_UNDER_DB / 20.0);
    for b in &mut bands {
        b.counted = b.rms > floor && b.rms > 0.0;
    }

    let counted: Vec<&BandCrest> = bands.iter().filter(|b| b.counted).collect();
    let widest = counted
        .iter()
        .max_by(|a, b| a.crest_db.total_cmp(&b.crest_db));
    let tightest = counted
        .iter()
        .min_by(|a, b| a.crest_db.total_cmp(&b.crest_db));

    Crest {
        broadband_crest_db: crest_db(
            samples.iter().fold(0.0, |a, x| a.max(x.abs())),
            rms(samples),
        ),
        spread_db: widest.zip(tightest).map(|(w, t)| w.crest_db - t.crest_db),
        widest_band_hz: widest.map(|b| b.centre_hz),
        tightest_band_hz: tightest.map(|b| b.centre_hz),
        counted_under_db: COUNTED_UNDER_DB,
        bands,
    }
}

/// A crest is a RATIO within one band, so the cascade's gain closed form cancels out of it.
fn band(samples: &[f64], sample_rate: f64, lo: f64, hi: f64) -> BandCrest {
    let centre = (lo * hi).sqrt();
    let q = centre / (hi - lo);
    let coeffs = design(Shape::Bandpass, centre, q, 0.0, sample_rate);
    let settle = settle_secs(centre, q) * sample_rate;
    let skip = (settle.round() as usize).min(samples.len());
    let (mut first, mut second) = (State::default(), State::default());
    let mut peak = 0.0f64;
    let mut power = 0.0f64;
    for (i, x) in samples.iter().enumerate() {
        let y = second.step(&coeffs, first.step(&coeffs, *x));
        if i < skip {
            continue;
        }
        peak = peak.max(y.abs());
        power += y * y;
    }
    let measured = samples.len() - skip;
    let rms = match measured {
        0 => 0.0,
        n => (power / n as f64).sqrt(),
    };
    BandCrest {
        lo_hz: lo,
        hi_hz: hi,
        centre_hz: centre,
        peak,
        rms,
        crest_db: crest_db(peak, rms),
        counted: false,
    }
}

fn settle_secs(centre_hz: f64, q: f64) -> f64 {
    SETTLE_TAUS * CASCADE_Q * q / (std::f64::consts::PI * centre_hz)
}

fn crest_db(peak: f64, rms: f64) -> f64 {
    if rms <= 0.0 { db(0.0) } else { db(peak / rms) }
}
