// Concern: measures the image two components carry, whole and per frame | Non-concern: level and spectrum per component (envelope.rs, spectrum.rs) | IO: (&[f64], &[f64], frame) -> StereoImage

use crate::measure::envelope::rms;

pub const RAIL_DB: f64 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoFrame {
    pub t_secs: f64,
    pub correlation: f64,
    pub mid_rms: f64,
    pub side_rms: f64,
    pub width: f64,
    pub balance_db: f64,
    pub mono_db: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StereoImage {
    pub channels: usize,
    pub overall: StereoFrame,
    pub frames: Vec<StereoFrame>,
}

pub fn analyze(
    planes: &[&[f64]],
    channels: usize,
    sample_rate: f64,
    start_secs: f64,
    frame_secs: f64,
) -> StereoImage {
    let (l, r) = (planes[0], planes[1]);
    let stride = ((frame_secs * sample_rate).round() as usize).max(1);
    let frames = l
        .chunks(stride)
        .zip(r.chunks(stride))
        .enumerate()
        .map(|(n, (li, ri))| measure(li, ri, start_secs + (n * stride) as f64 / sample_rate))
        .collect();
    StereoImage {
        channels,
        overall: measure(l, r, start_secs),
        frames,
    }
}

fn measure(l: &[f64], r: &[f64], t_secs: f64) -> StereoFrame {
    let mid: Vec<f64> = l.iter().zip(r).map(|(a, b)| (a + b) / 2.0).collect();
    let side: Vec<f64> = l.iter().zip(r).map(|(a, b)| (a - b) / 2.0).collect();
    let (mid_rms, side_rms) = (rms(&mid), rms(&side));
    let (l_rms, r_rms) = (rms(l), rms(r));
    StereoFrame {
        t_secs,
        correlation: pearson(l, r),
        mid_rms,
        side_rms,
        width: ratio(side_rms, mid_rms),
        balance_db: db(r_rms, l_rms),
        mono_db: db(mid_rms, (l_rms + r_rms) / 2.0),
    }
}

/// Silence over silence is 0, never NaN; something over silence is unbounded and says so.
fn ratio(a: f64, b: f64) -> f64 {
    match () {
        () if b > 0.0 => a / b,
        () if a > 0.0 => f64::INFINITY,
        () => 0.0,
    }
}

/// A rail rather than an infinity, so no reader has to special-case a null.
fn db(a: f64, b: f64) -> f64 {
    match ratio(a, b) {
        r if r > 0.0 => (20.0 * r.log10()).clamp(-RAIL_DB, RAIL_DB),
        _ if a > 0.0 => RAIL_DB,
        _ if b > 0.0 => -RAIL_DB,
        _ => 0.0,
    }
}

/// Pearson, so a gain difference does not read as decorrelation — `balance_db` is for that.
fn pearson(l: &[f64], r: &[f64]) -> f64 {
    let n = l.len().min(r.len());
    if n == 0 {
        return 1.0;
    }
    let mean = |v: &[f64]| v[..n].iter().copied().sum::<f64>() / n as f64;
    let (lm, rm) = (mean(l), mean(r));
    let (mut cov, mut lv, mut rv) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (a, b) = (l[i] - lm, r[i] - rm);
        cov += a * b;
        lv += a * a;
        rv += b * b;
    }
    if lv <= 0.0 || rv <= 0.0 {
        return 1.0;
    }
    (cov / (lv * rv).sqrt()).clamp(-1.0, 1.0)
}
