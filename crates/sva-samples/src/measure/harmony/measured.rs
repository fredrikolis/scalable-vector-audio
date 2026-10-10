// Concern: the partials of samples, a Hann frame centred on each instant, leakage peaks left out | Non-concern: picking peaks (spectrum.rs) | IO: (&[f64], rate, start, hop) -> moments, framing

use super::{Framing, Moment, Partial, Window};
use crate::measure::spectrum::{magnitudes, peaks, pinned_frame};

pub const WINDOW_SECS: f64 = 0.05;

pub const METHOD: &str = "measured: a Hann frame of `window.size` samples of the first \
                          component centred on each instant, zero past its ends; its local \
                          maxima picked as `spectrum` picks peaks, refined by the two-bin Hann \
                          interpolation of Grandke, T. (1983), IEEE Trans. Instrum. Meas. 32(2) \
                          350-355, exact for a lone sinusoid; a peak no louder than the Hann \
                          kernel's bound 1/(pi m (m^2 - 1)) at m bins (Harris, F. J. (1978), \
                          Proc. IEEE 66(1) 51-83) lets the louder peaks kept leak there is \
                          dropped as leakage. Each partial's bounds are Grandke's estimate over \
                          its two bins each widened by what that bound lets the other kept \
                          partials, at their estimated levels, leak into it: they hold where the \
                          frame is stationary sinusoids at the kept partials, and are a point \
                          for a lone one";

/// The bound METHOD states; within a bin of the peak, the peak itself.
fn leakage(m: f64) -> f64 {
    match m > 1.0 {
        true => (1.0 / (std::f64::consts::PI * m * (m * m - 1.0))).min(1.0),
        false => 1.0,
    }
}

/// The Hann kernel at an offset of `d` bins, |d| <= 1, against its peak.
fn kernel(d: f64) -> f64 {
    let d = d.abs();
    match d {
        0.0 => 1.0,
        _ if d >= 1.0 => 0.5,
        _ => {
            let x = std::f64::consts::PI * d;
            x.sin() / (x * (1.0 - d * d))
        }
    }
}

fn offset(a: f64) -> f64 {
    match a.is_finite() {
        true => ((2.0 * a - 1.0) / (a + 1.0)).clamp(-1.0, 1.0),
        false => 1.0,
    }
}

/// A peak's own bin and the side its louder neighbour stands on.
struct Peaked {
    k: usize,
    sign: f64,
    hz: f64,
    amplitude: f64,
}

/// Grandke's offset d = (2a - 1)/(a + 1) toward the louder neighbour, a = |X(k+-1)|/|X(k)|,
/// and amplitude |X(k)| over the kernel there.
fn refined(mags: &[f64], bin_hz: f64, hz: f64) -> Peaked {
    let near = (hz / bin_hz).round() as usize;
    let k = (near.saturating_sub(1)..=near + 1)
        .filter(|k| (1..mags.len() - 1).contains(k))
        .max_by(|a, b| mags[*a].total_cmp(&mags[*b]))
        .expect("a peak stands off both ends");
    let sign = match mags[k + 1] >= mags[k - 1] {
        true => 1.0,
        false => -1.0,
    };
    let d = offset(mags[(k as f64 + sign) as usize] / mags[k]);
    Peaked {
        k,
        sign,
        hz: (k as f64 + sign * d) * bin_hz,
        amplitude: mags[k] / kernel(d),
    }
}

/// The same estimate over each of its two bins widened by `leaked`, what the other kept
/// partials can put there: the bounds METHOD states.
fn bounded(p: &Peaked, mags: &[f64], bin_hz: f64, leaked: impl Fn(f64) -> f64) -> Partial {
    let toward = p.k as f64 + p.sign;
    let widened = |x: f64, by: f64| ((x - by).max(0.0), x + by);
    let own = widened(mags[p.k], leaked(p.k as f64));
    let side = widened(mags[toward as usize], leaked(toward));
    let a_hi = match own.0 > 0.0 {
        true => side.1 / own.0,
        false => f64::INFINITY,
    };
    let d = (offset(side.0 / own.1), offset(a_hi));
    let ends = [
        (p.k as f64 + p.sign * d.0) * bin_hz,
        (p.k as f64 + p.sign * d.1) * bin_hz,
    ];
    let nearest = match d.0 <= 0.0 && d.1 >= 0.0 {
        true => 0.0,
        false => d.0.abs().min(d.1.abs()),
    };
    Partial {
        hz: p.hz,
        amplitude: p.amplitude,
        hz_bounds: (ends[0].min(ends[1]), ends[0].max(ends[1])),
        amplitude_bounds: (
            own.0 / kernel(nearest),
            own.1 / kernel(d.0.abs().max(d.1.abs())),
        ),
    }
}

pub fn moments(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    hop_secs: f64,
) -> (Vec<Moment>, Framing) {
    let hop = ((hop_secs * sample_rate).round() as usize).max(1);
    let size = pinned_frame(WINDOW_SECS, sample_rate);
    let half = (size / 2) as i64;
    let mut held = Vec::new();
    for centre in (0..samples.len()).step_by(hop) {
        let frame: Vec<f64> = (0..size as i64)
            .map(|k| {
                let at = centre as i64 - half + k;
                usize::try_from(at)
                    .ok()
                    .and_then(|at| samples.get(at))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect();
        let (mags, bin_hz, ..) = magnitudes(&frame, sample_rate, Some(WINDOW_SECS));
        let mut kept: Vec<Peaked> = Vec::new();
        for peak in peaks(&mags, bin_hz, usize::MAX) {
            let found = refined(&mags, bin_hz, peak.hz);
            let leaked: f64 = kept
                .iter()
                .map(|q| q.amplitude * leakage((found.hz - q.hz).abs() / bin_hz))
                .sum();
            if found.amplitude > leaked {
                kept.push(found);
            }
        }
        let partials = kept
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let leaked = |bin: f64| -> f64 {
                    kept.iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, q)| q.amplitude * leakage((bin - q.hz / bin_hz).abs()))
                        .sum()
                };
                bounded(p, &mags, bin_hz, leaked)
            })
            .collect();
        held.push(Moment {
            t_secs: start_secs + centre as f64 / sample_rate,
            partials,
        });
    }
    let framing = Framing {
        hop_secs: hop as f64 / sample_rate,
        window: Some(Window {
            size,
            secs: size as f64 / sample_rate,
            resolution_hz: sample_rate / size as f64,
        }),
    };
    (held, framing)
}
