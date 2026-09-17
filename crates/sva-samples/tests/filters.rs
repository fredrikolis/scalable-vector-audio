// Concern: states what one filter call site owes a stream and its own parameter trace | Non-concern: coefficient math (src/biquad.rs) | IO: (a swept site) -> asserted dB, frames

use sva_samples::Shape;
use sva_samples::filters::FilterSite;

fn site(shape: Shape, cutoff: f64, q: f64, sr: f64) -> FilterSite {
    FilterSite::new(shape, 1, &[cutoff], &[q], &[0.0], sr)
}

fn step(s: &mut FilterSite, x: f64, cutoff: f64, q: f64, gain: f64, sr: f64, i: usize) -> f64 {
    let mut out = [0.0];
    s.process(&[x], &[cutoff], &[q], &[gain], &mut out, sr, i);
    out[0]
}

fn run(shape: Shape, cutoff: f64, q: f64, input: &[f64]) -> Vec<f64> {
    let sr = 48000.0;
    let mut s = site(shape, cutoff, q, sr);
    input
        .iter()
        .enumerate()
        .map(|(i, &x)| step(&mut s, x, cutoff, q, 0.0, sr, i))
        .collect()
}

fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

fn tone(hz: f64, n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin())
        .collect()
}

/// The measured attenuation must match what magnitude_db predicted, or the report lies.
#[test]
fn a_rendered_tone_is_attenuated_by_what_the_response_predicts() {
    let n = 48000;
    for (shape, hz, cutoff) in [
        (Shape::Lowpass, 6000.0, 500.0),
        (Shape::Highpass, 200.0, 2000.0),
        (Shape::Bandpass, 1000.0, 1000.0),
    ] {
        let out = run(shape, cutoff, 0.707, &tone(hz, n));
        let settled = &out[n / 2..];
        let measured = 20.0 * (rms(settled) / rms(&tone(hz, n)[n / 2..])).log10();
        let predicted = sva_samples::biquad::magnitude_db(
            &sva_samples::biquad::design(shape, cutoff, 0.707, 0.0, 48000.0),
            hz,
            48000.0,
        );
        assert!(
            (measured - predicted).abs() < 0.5,
            "{}: measured {measured:.2} dB, predicted {predicted:.2} dB",
            shape.name()
        );
    }
}

#[test]
fn a_high_q_lowpass_rings_above_unity_at_its_corner() {
    let n = 48000;
    let out = run(Shape::Lowpass, 800.0, 6.0, &tone(800.0, n));
    assert!(rms(&out[n / 2..]) > 4.0, "Q=6 should boost the corner");
    assert!(out.iter().all(|v| v.is_finite()));
}

/// A range says a filter swept; a trace says WHEN each value happened.
#[test]
fn a_swept_cutoff_is_traced_in_time_and_sliceable_by_window() {
    let sr = 48000.0;
    let mut s = site(Shape::Lowpass, 200.0, 1.0, sr);
    for i in 0..96000 {
        step(&mut s, 0.1, 200.0 + i as f64 / 48.0, 1.0, 0.0, sr, i);
    }
    let traces = s.trace("n", 0, sr);
    assert_eq!(traces.len(), 1, "a mono site is one trace");
    let trace = &traces[0];
    assert_eq!(trace.channel, None, "and states no channel");
    assert_eq!(trace.frames.len(), 2000, "two seconds at 1kHz trace rate");

    let second = trace.over(1.0, 2.0, sr);
    assert_eq!(second.frames.len(), 1000);
    assert!((second.frames[0].t_secs - 1.0).abs() < 1e-9);
    assert!(
        (second.frames[0].cutoff - 1200.0).abs() < 1.0,
        "the sweep's value AT one second: {:?}",
        second.frames[0]
    );
    assert_ne!(
        second.coefficients,
        trace.over(0.0, 1.0, sr).coefficients,
        "each window resolves its own equation"
    );

    let sliver = trace.over(1.50041, 1.50049, sr);
    assert!(sliver.frames.is_empty(), "narrower than the trace spacing");
    assert_eq!(
        sliver.coefficients,
        trace.over(1.4995, 1.5005, sr).coefficients,
        "a window with no frame of its own takes the nearest one, not the first"
    );
    assert_ne!(sliver.coefficients, trace.over(0.0, 0.001, sr).coefficients);

    // Four frames straddling a midpoint of 1.0024: nearest is 1.002, array-middle 1.003.
    let lopsided = trace.over(1.0005, 1.0043, sr);
    assert_eq!(lopsided.frames.len(), 4);
    assert_eq!(
        lopsided.coefficients,
        trace.over(1.0015, 1.0025, sr).coefficients,
        "distance to the midpoint decides"
    );
    assert_ne!(
        lopsided.coefficients,
        trace.over(1.0025, 1.0035, sr).coefficients,
        "array position does not"
    );
}

#[test]
fn a_swept_gain_is_traced_alongside_cutoff_and_q() {
    let sr = 48000.0;
    let mut s = site(Shape::Peaking, 1000.0, 2.0, sr);
    for i in 0..48000 {
        step(&mut s, 0.1, 1000.0, 2.0, i as f64 / 4800.0, sr, i);
    }
    let frames = s.trace("n", 0, sr)[0].frames.clone();
    assert_eq!(frames.first().map(|f| f.gain_db), Some(0.0));
    assert!(frames.last().unwrap().gain_db > 9.9);
    assert!(frames.iter().all(|f| f.cutoff == 1000.0 && f.q == 2.0));
}

#[test]
fn a_cutoff_swept_past_nyquist_is_flagged_clamped_and_stays_finite() {
    let sr = 48000.0;
    let mut s = site(Shape::Highpass, 1000.0, 1.0, sr);
    for i in 0..100 {
        let y = step(&mut s, 0.5, 1000.0 + i as f64 * 1000.0, 1.0, 0.0, sr, i);
        assert!(y.is_finite());
    }
    assert!(s.trace("n", 0, sr)[0].clamped);
}

/// One call site, two independent delay lines and two independent equations.
#[test]
fn a_wide_cutoff_gives_one_site_a_lane_and_a_trace_per_component() {
    let sr = 48000.0;
    let cutoff = [400.0, 6000.0];
    let (q, gain) = ([0.707], [0.0]);
    let mut s = FilterSite::new(Shape::Lowpass, 2, &cutoff, &q, &gain, sr);

    let n = 24000;
    let out: Vec<[f64; 2]> = tone(3000.0, n)
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let mut lanes = [0.0; 2];
            s.process(&[x], &cutoff, &q, &gain, &mut lanes, sr, i);
            lanes
        })
        .collect();
    let settled = |c: usize| rms(&out[n / 2..].iter().map(|v| v[c]).collect::<Vec<_>>());
    assert!(
        settled(0) * 4.0 < settled(1),
        "the 400 Hz lane must cut 3 kHz far harder than the 6 kHz one: {} vs {}",
        settled(0),
        settled(1)
    );

    let traces = s.trace("n", 0, sr);
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[0].channel, Some(0));
    assert_eq!(traces[1].frames[0].cutoff, 6000.0);
}
