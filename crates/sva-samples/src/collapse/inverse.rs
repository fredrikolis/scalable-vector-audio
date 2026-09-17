// Concern: takes a closed form in f to the grid by sampling it at 1/T and one inverse transform | Non-concern: closed forms in t (point.rs, lines.rs) | IO: (&SpectralSum, rate) -> planes, wrap dB

use std::f64::consts::TAU;

use sva_formula::{C64, SpectralSum};

use crate::error::CollapseError;
use crate::fft::idft;

use super::lines::bins;
use super::point::eval_lane;

/// Ours, not a bound the table sets.
const IMAGES_COUNTED: u32 = 3;

/// Bin `j` stands for `j/T` Hz: the closed form is read at exactly the spacing the horizon names.
pub fn collapse_lane(
    n: &SpectralSum,
    c: usize,
    start_secs: f64,
    horizon_secs: f64,
    rate: u32,
    len: usize,
) -> Result<Vec<f64>, CollapseError> {
    let count = bins(horizon_secs, rate);
    let spacing = 1.0 / horizon_secs;
    let lane = &n.lanes[c.min(n.lanes.len() - 1)];
    let mut re = vec![0.0; count];
    let mut im = vec![0.0; count];
    for j in 0..count {
        let hz = bin_hz(j, count, spacing);
        let v = eval_lane(lane, hz)? * C64::new(0.0, TAU * hz * start_secs).exp();
        re[j] = v.re * spacing;
        im[j] = v.im * spacing;
    }
    idft(&mut re, &mut im);
    let scale = count as f64;
    let mut out: Vec<f64> = re.iter().map(|x| x * scale).collect();
    out.truncate(len);
    out.resize(len, 0.0);
    Ok(out)
}

/// The energy outside `[-R/2, R/2]`, which the grid folds back in.
pub fn wrap_db(n: &SpectralSum, horizon_secs: f64, rate: u32) -> Result<f64, CollapseError> {
    let count = bins(horizon_secs, rate);
    let spacing = 1.0 / horizon_secs;
    let lane = &n.lanes[0];
    let (mut held, mut lost) = (0.0, 0.0);
    for j in 0..count {
        let hz = bin_hz(j, count, spacing);
        held += eval_lane(lane, hz)?.norm_sqr();
        for k in 1..=IMAGES_COUNTED {
            let shift = f64::from(k) * f64::from(rate);
            lost += eval_lane(lane, hz + shift)?.norm_sqr();
            lost += eval_lane(lane, hz - shift)?.norm_sqr();
        }
    }
    Ok(match (held, lost) {
        (_, 0.0) => f64::NEG_INFINITY,
        (0.0, _) => 0.0,
        _ => 10.0 * (lost / held).log10(),
    })
}

fn bin_hz(j: usize, count: usize, spacing: f64) -> f64 {
    let signed = if j <= count / 2 {
        j as f64
    } else {
        j as f64 - count as f64
    };
    signed * spacing
}
