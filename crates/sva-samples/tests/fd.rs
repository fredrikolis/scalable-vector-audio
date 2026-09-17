// Concern: drives one finite-difference solver into a buffer and reads its spectrum back | Non-concern: what any one model must hold (the physics_* files) | IO: (Params, rate, secs) -> Buffer, peaks

use sva_samples::measure::spectrum;
use sva_samples::{Buffer, Params, site};

pub fn render(params: &Params, rate: u32, secs: f64) -> Buffer {
    assert!(
        params.valid(),
        "{} got parameters it refuses",
        params.name()
    );
    let len = (secs * f64::from(rate)).round() as usize;
    let mut solver = site(params, rate).expect("a grid this rate can hold");
    Buffer::mono(rate, (0..len).map(|_| solver.step()).collect())
}

pub fn peaks(buffer: &Buffer, max_peaks: usize, frame_secs: Option<f64>) -> Vec<f64> {
    let s = spectrum::analyze(
        buffer.plane(0),
        f64::from(buffer.rate),
        max_peaks,
        frame_secs,
    );
    s.peaks.iter().map(|p| p.hz).collect()
}

pub fn nearest(peaks: &[f64], hz: f64) -> f64 {
    peaks
        .iter()
        .copied()
        .min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs()))
        .expect("a spectrum with peaks")
}

pub fn sounds(buffer: &Buffer) {
    assert!(
        buffer.plane(0).iter().all(|s| s.is_finite()),
        "a diverged grid tests nothing"
    );
    assert!(
        buffer.plane(0).iter().any(|&s| s != 0.0),
        "silence tests nothing"
    );
}

pub fn is_deterministic(params: &Params, rate: u32, secs: f64) {
    let once = render(params, rate, secs);
    sounds(&once);
    assert_eq!(once, render(params, rate, secs));
}

pub fn refuses(base: &Params, broken: Vec<Params>) {
    assert!(
        base.valid(),
        "{} refuses its own reference set",
        base.name()
    );
    for p in broken {
        assert!(!p.valid(), "{} admitted {p:?}", p.name());
    }
}

/// The observable half of the deleted discrete-energy and Courant sweeps: an undamped grid
/// that conserves energy under a stable step neither blows up nor decays away.
pub fn stays_bounded(params: &Params, rate: u32, secs: f64) {
    let buffer = render(params, rate, secs);
    sounds(&buffer);
    let plane = buffer.plane(0);
    let peak = plane.iter().fold(0.0f64, |m, &s| m.max(s.abs()));
    let late = plane[plane.len() / 2..]
        .iter()
        .fold(0.0f64, |m, &s| m.max(s.abs()));
    assert!(
        late <= peak * 1.000_001,
        "{}: the second half peaks at {late}, above the whole run's {peak}",
        params.name()
    );
}
