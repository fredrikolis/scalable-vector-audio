// Concern: the forward and inverse short-time transform between a buffer and frames | Non-concern: holding either (buffer.rs, frames.rs) | IO: (Buffer, w, h) <-> Frames

use std::f64::consts::TAU;

use crate::buffer::Buffer;
use crate::error::SampleError;
use crate::fft::{fft, irfft};
use crate::frames::Frames;
use crate::label::{Detail, Label, Rule, Source};
use crate::profile::Profile;

/// Periodic, not symmetric: the symmetric window's duplicated endpoint is what breaks
/// constant overlap-add at a hop that divides the window.
pub fn hann_periodic(n: usize) -> Vec<f64> {
    (0..n)
        .map(|k| 0.5 - 0.5 * (TAU * k as f64 / n as f64).cos())
        .collect()
}

const COLA_TOL: f64 = 1e-12;

pub fn cola_ok(window: &[f64], hop: usize) -> bool {
    if hop == 0 || hop > window.len() {
        return false;
    }
    let sums: Vec<f64> = (0..hop)
        .map(|phase| {
            window
                .iter()
                .skip(phase)
                .step_by(hop)
                .map(|w| w * w)
                .sum::<f64>()
        })
        .collect();
    let first = sums[0];
    first > COLA_TOL
        && sums
            .iter()
            .all(|s| (s - first).abs() <= COLA_TOL * first.max(1.0))
}

fn frame_count(samples: usize, window: usize, hop: usize) -> usize {
    let span = samples + 2 * (window - hop);
    span.div_ceil(hop)
}

fn start_of(frame: usize, window: usize, hop: usize) -> isize {
    frame as isize * hop as isize - (window as isize - hop as isize)
}

pub fn forward(x: &Buffer, window: usize, hop: usize) -> Result<Frames, SampleError> {
    if !window.is_power_of_two() {
        return Err(SampleError::WindowNotPowerOfTwo { window });
    }
    let w = hann_periodic(window);
    if !cola_ok(&w, hop) {
        return Err(SampleError::HopOutsideCola { window, hop });
    }
    let samples = x.len();
    let count = frame_count(samples, window, hop);
    let mut out = Frames::silence(x.rate, window, hop, x.width, count, samples);
    out.origin_secs = x.origin_secs;
    for c in 0..x.width {
        let plane = x.plane(c);
        for frame in 0..count {
            let start = start_of(frame, window, hop);
            let mut re = vec![0.0; window];
            let mut im = vec![0.0; window];
            for (n, slot) in re.iter_mut().enumerate() {
                let at = start + n as isize;
                let s = usize::try_from(at)
                    .ok()
                    .and_then(|i| plane.get(i).copied())
                    .unwrap_or(0.0);
                *slot = s * w[n];
            }
            fft(&mut re, &mut im);
            for bin in 0..out.bins {
                out.place(c, frame, bin, re[bin], im[bin]);
            }
        }
    }
    Ok(out)
}

/// Accumulates the overlap-added signal and the summed window square in one pass, then
/// divides one by the other per sample. That makes the leading and trailing edges exact
/// instead of assuming a steady-state overlap sum.
pub fn inverse(fr: &Frames, profile: &Profile) -> (Buffer, Label) {
    let w = hann_periodic(fr.window);
    let mut planes = Vec::with_capacity(fr.width);
    for c in 0..fr.width {
        let mut y = vec![0.0; fr.samples];
        let mut d = vec![0.0; fr.samples];
        for frame in 0..fr.frames {
            let start = start_of(frame, fr.window, fr.hop);
            let (re, im): (Vec<f64>, Vec<f64>) =
                (0..fr.bins).map(|bin| fr.at(c, frame, bin)).unzip();
            let block = irfft(&re, &im, fr.window);
            for n in 0..fr.window {
                let Ok(at) = usize::try_from(start + n as isize) else {
                    continue;
                };
                if at >= fr.samples {
                    break;
                }
                y[at] += block[n] * w[n];
                d[at] += w[n] * w[n];
            }
        }
        for (sample, weight) in y.iter_mut().zip(&d) {
            if *weight > COLA_TOL {
                *sample /= weight;
            }
        }
        planes.push(y);
    }
    let mut buffer = Buffer::of_planes(fr.rate, planes);
    buffer.origin_secs = fr.origin_secs;
    let source = if fr.edited {
        Source::Measured
    } else {
        Source::Exact
    };
    let label = Label::new(
        source,
        profile.name,
        fr.rate,
        Detail::Roundtrip {
            rule: Rule::Istft,
            edited: fr.edited,
        },
    );
    (buffer, label)
}
