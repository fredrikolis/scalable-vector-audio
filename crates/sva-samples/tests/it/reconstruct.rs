// Concern: states that each kernel of the family reads between samples inside its bound | Non-concern: where a reading's positions come from | IO: (length, fraction, frequency) -> asserted error

use std::f64::consts::PI;

use sva_samples::{Kernel, PSYCHOACOUSTIC_V1, kernel, plain};

/// The response a reading at fraction `f` gives a unit component at `w` radians per sample.
fn response(k: &Kernel, f: f64, w: f64) -> (f64, f64) {
    let mut taps = Vec::new();
    k.weights(f, &mut taps);
    let n = k.half_width() as i64;
    taps.iter()
        .zip((1 - n)..=n)
        .fold((0.0, 0.0), |(re, im), (k, j)| {
            let phase = w * (j as f64 - f);
            (re + k * phase.cos(), im + k * phase.sin())
        })
}

/// The worst error over a band, and the largest sum of weights seen.
fn worst(k: &Kernel, band: std::ops::Range<f64>) -> (f64, f64) {
    let (mut out, mut sum) = (0.0f64, 0.0f64);
    let mut taps = Vec::new();
    for a in 1..64 {
        let f = (f64::from(a) / 64.0 + 1.0 / 997.0).fract();
        k.weights(f, &mut taps);
        sum = sum.max(taps.iter().map(|w| w.abs()).sum());
        for b in 0..=160 {
            let w = 2.0 * PI * (band.start + (band.end - band.start) * f64::from(b) / 160.0);
            let (re, im) = response(k, f, w);
            out = out.max((re - 1.0).hypot(im));
        }
    }
    (out, sum)
}

#[test]
fn the_plain_kernel_meets_its_own_precision_below_the_band_edge() {
    let p = PSYCHOACOUSTIC_V1;
    let bound = plain()
        .bound(p.ceiling_hz, p.lattice_hz)
        .expect("the profile's band edge clears the main lobe");
    assert!(
        bound.in_band <= p.half_lsb(),
        "in band {} past half an lsb {}",
        bound.in_band,
        p.half_lsb()
    );
    assert!(bound.above_band < 1.0 + 1e-6, "{}", bound.above_band);
}

#[test]
fn each_length_errs_less_than_its_bound_on_either_side_of_the_band_edge() {
    let p = PSYCHOACOUSTIC_V1;
    let edge = p.ceiling_hz / f64::from(p.lattice_hz);
    for n in [16, 60, 84, 128] {
        let k = kernel(n);
        let bound = k.bound(p.ceiling_hz, p.lattice_hz).expect("a bound");
        let (below, sum) = worst(k, 0.0..edge);
        let (above, _) = worst(k, edge..0.5);
        assert!(
            below <= bound.in_band,
            "{n}: {below} past {}",
            bound.in_band
        );
        assert!(
            above <= bound.above_band,
            "{n}: {above} past {}",
            bound.above_band
        );
        assert!(sum <= bound.lebesgue, "{n}: {sum} past {}", bound.lebesgue);
    }
}

#[test]
fn a_longer_kernel_reads_closer() {
    let p = PSYCHOACOUSTIC_V1;
    let at = |n| {
        kernel(n)
            .bound(p.ceiling_hz, p.lattice_hz)
            .expect("a bound")
            .in_band
    };
    assert!(at(84) < at(60) / 100.0, "{} against {}", at(84), at(60));
}

#[test]
fn a_band_edge_inside_the_main_lobe_has_no_bound() {
    assert!(plain().bound(20_000.0, 42_000).is_none());
}
