// Concern: where one partial of a rendered string rings, and how loud | Non-concern: rendering it (fd.rs) | IO: (samples, rate, Hz) -> Hz, dB

use sva_samples::measure::spectrum;

/// One Hann-windowed DFT bin.
pub fn level_db(x: &[f64], rate: f64, hz: f64, start: usize, len: usize) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    for (i, s) in x[start..start + len].iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / len as f64).cos();
        let phase = std::f64::consts::TAU * hz * (start + i) as f64 / rate;
        re += w * s * phase.cos();
        im -= w * s * phase.sin();
    }
    10.0 * (re * re + im * im).log10()
}

/// The peak bin near `hz`, refined by golden section.
pub fn ringing_hz(x: &[f64], rate: f64, hz: f64) -> f64 {
    let (mags, bin_hz, _, _) = spectrum::magnitudes(x, rate, None);
    let lo = ((hz * 0.97) / bin_hz) as usize;
    let hi = ((hz * 1.03) / bin_hz) as usize;
    let peak = (lo..=hi)
        .max_by(|&a, &b| mags[a].total_cmp(&mags[b]))
        .expect("a band with bins") as f64
        * bin_hz;
    let start = (0.05 * rate) as usize;
    let level = |f: f64| level_db(x, rate, f, start, x.len() - start);
    let golden = (5f64.sqrt() - 1.0) / 2.0;
    let (mut a, mut b) = (peak - bin_hz, peak + bin_hz);
    while b - a > 1e-6 {
        let (c, d) = (b - golden * (b - a), a + golden * (b - a));
        if level(c) > level(d) {
            b = d;
        } else {
            a = c;
        }
    }
    (a + b) / 2.0
}

/// Partial `k` of a stiff string: `k f0 sqrt(1 + b k^2)`.
pub fn stiff_partial(f0: f64, b: f64, k: u32) -> f64 {
    f64::from(k) * f0 * (1.0 + b * f64::from(k * k)).sqrt()
}
