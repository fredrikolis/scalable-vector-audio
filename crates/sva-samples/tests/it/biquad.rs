// Concern: states what the bilinear biquad designs owe the shapes they are named for | Non-concern: H(s) on a closed form (sva-formula) | IO: (Shape, cutoff, q, gain) -> asserted dB

use sva_samples::Shape;
use sva_samples::biquad::{RESPONSE_RATIOS, clamp_cutoff, clamp_q, design, magnitude_db, response};

const SR: f64 = 48000.0;

#[test]
fn all_shapes_round_trips_through_name_and_from_name() {
    for shape in sva_formula::filter::ALL_SHAPES {
        assert_eq!(Shape::from_name(shape.name()), Some(shape));
    }
}

fn db_at(shape: Shape, cutoff: f64, q: f64, hz: f64) -> f64 {
    magnitude_db(&design(shape, cutoff, q, 0.0, SR), hz, SR)
}

#[test]
fn every_shape_puts_its_energy_where_its_name_says() {
    assert!(db_at(Shape::Lowpass, 1000.0, 0.707, 100.0) > -0.5);
    assert!(db_at(Shape::Lowpass, 1000.0, 0.707, 8000.0) < -30.0);

    assert!(db_at(Shape::Highpass, 1000.0, 0.707, 8000.0) > -0.5);
    assert!(db_at(Shape::Highpass, 1000.0, 0.707, 100.0) < -30.0);

    assert!(db_at(Shape::Bandpass, 1000.0, 2.0, 1000.0).abs() < 0.1);
    assert!(db_at(Shape::Bandpass, 1000.0, 2.0, 100.0) < -20.0);
    assert!(db_at(Shape::Bandpass, 1000.0, 2.0, 10000.0) < -20.0);

    assert!(db_at(Shape::Notch, 1000.0, 2.0, 1000.0) < -60.0);
    assert!(db_at(Shape::Notch, 1000.0, 2.0, 100.0).abs() < 0.5);
}

/// The whole point of Q: a resonant corner is a real peak above unity gain.
#[test]
fn q_raises_the_corner_and_the_butterworth_default_does_not() {
    let flat = db_at(Shape::Lowpass, 1000.0, 0.707, 1000.0);
    assert!((flat + 3.0).abs() < 0.5, "-3 dB at the corner, got {flat}");
    let resonant = db_at(Shape::Lowpass, 1000.0, 8.0, 1000.0);
    assert!(resonant > 15.0, "Q=8 should ring, got {resonant}");
}

#[test]
fn a_shelf_and_a_bell_reach_their_stated_gain() {
    let peak = magnitude_db(&design(Shape::Peaking, 1000.0, 1.0, 9.0, SR), 1000.0, SR);
    assert!((peak - 9.0).abs() < 0.1, "peaking gain {peak}");

    let low = magnitude_db(&design(Shape::Lowshelf, 1000.0, 0.707, -12.0, SR), 20.0, SR);
    assert!((low + 12.0).abs() < 0.3, "lowshelf gain {low}");

    let high = magnitude_db(
        &design(Shape::Highshelf, 1000.0, 0.707, 6.0, SR),
        20000.0,
        SR,
    );
    assert!((high - 6.0).abs() < 0.3, "highshelf gain {high}");
}

/// `lp` reported in biquad form must still be the one-pole `y += alpha*(x - y)`.
#[test]
fn the_one_pole_is_a_degenerate_biquad_at_minus_three_db() {
    let c = design(Shape::OnePole, 1000.0, 0.0, 0.0, SR);
    assert_eq!((c.b1, c.b2, c.a2), (0.0, 0.0, 0.0));
    assert!(
        (c.b0 - (1.0 + c.a1)).abs() < 1e-12,
        "b0 = alpha, a1 = alpha-1"
    );
    let corner = magnitude_db(&c, 1000.0, SR);
    assert!((corner + 3.0).abs() < 0.3, "one-pole corner {corner}");
    let octave_up = magnitude_db(&c, 2000.0, SR);
    assert!(
        (corner - octave_up - 4.0).abs() < 1.5,
        "6 dB/oct, not 12: {corner} -> {octave_up}"
    );
}

#[test]
fn the_response_table_stops_at_nyquist_instead_of_piling_up_on_it() {
    let full = response(&design(Shape::Lowpass, 500.0, 1.0, 0.0, SR), 500.0, SR);
    assert_eq!(full.len(), RESPONSE_RATIOS.len());

    let high = response(&design(Shape::Lowpass, 8000.0, 1.0, 0.0, SR), 8000.0, SR);
    assert_eq!(high.len(), 7, "4x and 8x of 8kHz are past 24kHz");
    assert!(high.iter().all(|(hz, db)| *hz < SR / 2.0 && *db > -200.0));
}

#[test]
fn a_cutoff_past_nyquist_or_at_zero_clamps_rather_than_producing_nonsense() {
    let (hz, clamped) = clamp_cutoff(30000.0, SR);
    assert!(clamped && hz < SR / 2.0);
    assert_eq!(clamp_cutoff(0.0, SR), (1.0, true));
    assert_eq!(clamp_cutoff(1000.0, SR), (1000.0, false));
    for shape in [Shape::Lowpass, Shape::Highpass, Shape::Bandpass] {
        let c = design(shape, hz, clamp_q(0.0).0, 0.0, SR);
        assert!(c.b0.is_finite() && c.a1.is_finite() && c.a2.is_finite());
    }
}
