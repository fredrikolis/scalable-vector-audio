// Concern: states that a kernel reading between lattice samples stays inside the bound it reports | Non-concern: where a reading's positions come from | IO: (fraction, frequency) -> asserted error

use std::f64::consts::PI;

use sva_samples::{PSYCHOACOUSTIC_V1, kernel};

/// The response a reading at fraction `f` gives a unit component at `w` radians per sample.
fn response(f: f64, w: f64) -> (f64, f64) {
    let mut taps = Vec::new();
    kernel().weights(f, &mut taps);
    let n = kernel().half_width() as i64;
    taps.iter()
        .zip((1 - n)..=n)
        .fold((0.0, 0.0), |(re, im), (k, j)| {
            let phase = w * (j as f64 - f);
            (re + k * phase.cos(), im + k * phase.sin())
        })
}

fn worst(band: std::ops::Range<f64>) -> f64 {
    let mut out = 0.0f64;
    for a in 1..64 {
        let f = f64::from(a) / 64.0 + 1.0 / 997.0;
        for b in 0..=160 {
            let w = 2.0 * PI * (band.start + (band.end - band.start) * f64::from(b) / 160.0);
            let (re, im) = response(f.fract(), w);
            out = out.max((re - 1.0).hypot(im));
        }
    }
    out
}

#[test]
fn the_profile_kernel_meets_its_own_precision_below_the_band_edge() {
    let p = PSYCHOACOUSTIC_V1;
    let bound = kernel()
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
fn a_reading_errs_less_than_its_bound_on_either_side_of_the_band_edge() {
    let p = PSYCHOACOUSTIC_V1;
    let bound = kernel().bound(p.ceiling_hz, p.lattice_hz).expect("a bound");
    let edge = p.ceiling_hz / f64::from(p.lattice_hz);
    let below = worst(0.0..edge);
    let above = worst(edge..0.5);
    assert!(below <= bound.in_band, "{below} past {}", bound.in_band);
    assert!(
        above <= bound.above_band,
        "{above} past {}",
        bound.above_band
    );
}

#[test]
fn a_band_edge_inside_the_main_lobe_has_no_bound() {
    assert!(kernel().bound(20_000.0, 42_000).is_none());
}
