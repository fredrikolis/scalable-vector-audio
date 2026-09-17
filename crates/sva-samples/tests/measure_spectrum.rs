// Concern: states what the discrete spectrum owes tones of known content | Non-concern: the FFT itself (src/fft.rs), the ERB bank (measure_bands.rs) | IO: (samples) -> asserted Hz and dB

use sva_samples::measure::spectrum::{analyze, db, third_octave_edges};

const RATE: f64 = 44_100.0;

fn tone(hz: f64, secs: f64, amp: f64) -> Vec<f64> {
    let len = (secs * RATE) as usize;
    (0..len)
        .map(|i| amp * (std::f64::consts::TAU * hz * i as f64 / RATE).sin())
        .collect()
}

/// A sine's RMS is `amp/sqrt(2)` and every other figure names the one partial it holds.
#[test]
fn one_sine_reads_as_its_own_frequency_level_and_centroid() {
    let s = analyze(&tone(1000.0, 1.0, 0.5), RATE, 4, None);
    assert!(s.frames >= 1 && s.frame_size.is_power_of_two());
    assert!(
        (s.rms - 0.5 / 2f64.sqrt()).abs() < 0.01,
        "a sine's RMS is its amplitude over root two: {}",
        s.rms
    );
    let loudest = s.peaks.first().expect("a sine has a peak");
    assert!(
        (loudest.hz - 1000.0).abs() < s.resolution_hz,
        "within one bin of 1 kHz: {loudest:?}"
    );
    assert!(
        (s.centroid_hz - 1000.0).abs() < 50.0,
        "one partial is its own centre of mass: {}",
        s.centroid_hz
    );
    assert!(
        s.rolloff85_hz >= 1000.0 - s.resolution_hz && s.rolloff85_hz < 2000.0,
        "85% of the energy is at or below the partial: {}",
        s.rolloff85_hz
    );
}

/// Two partials an octave apart, the upper half the amplitude.
#[test]
fn two_partials_are_both_found_and_ordered_by_level() {
    let low = tone(500.0, 1.0, 0.5);
    let high = tone(1000.0, 1.0, 0.25);
    let mixed: Vec<f64> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
    let s = analyze(&mixed, RATE, 4, None);

    let [first, second, ..] = s.peaks.as_slice() else {
        panic!("two partials, two peaks: {:?}", s.peaks);
    };
    assert!((first.hz - 500.0).abs() < s.resolution_hz, "{first:?}");
    assert!((second.hz - 1000.0).abs() < s.resolution_hz, "{second:?}");
    assert!(first.db > second.db, "the louder partial is named first");
    assert!(
        (first.db - second.db - 6.02).abs() < 0.5,
        "half the amplitude is 6 dB down: {first:?} {second:?}"
    );
    assert!(s.centroid_hz > 500.0 && s.centroid_hz < 1000.0);
}

/// Silence has no peak to name and no level to state, and says so rather than guessing.
#[test]
fn silence_reads_as_the_floor_with_no_peaks() {
    let s = analyze(&vec![0.0; 4096], RATE, 4, None);
    assert_eq!(s.rms, 0.0);
    assert!(s.peaks.is_empty(), "{:?}", s.peaks);
    assert_eq!(db(0.0), db(0.0), "the floor is one value, not a NaN");
    assert!(db(0.0) < -100.0, "and it is a floor: {}", db(0.0));
}

/// Every band reading downstream is sliced on these.
#[test]
fn third_octave_edges_step_by_a_third_of_an_octave_up_to_nyquist() {
    let edges = third_octave_edges(RATE);
    let [(lo, hi), ..] = edges.as_slice() else {
        panic!("at least one band");
    };
    assert!((hi / lo - 2f64.powf(1.0 / 3.0)).abs() < 1e-9, "{lo} {hi}");
    let (_, top) = edges.last().expect("a last band");
    assert!(*top <= RATE / 2.0, "nothing above Nyquist: {top}");
    for pair in edges.windows(2) {
        assert_eq!(pair[0].1, pair[1].0, "the bands tile without a gap");
    }
}

/// A lone tone puts its energy in the band that holds it, and leaves the others far below.
#[test]
fn a_bands_level_is_the_energy_of_the_partials_inside_it() {
    let s = analyze(&tone(1000.0, 1.0, 0.5), RATE, 4, None);
    let holding = s
        .bands
        .iter()
        .find(|b| b.lo_hz <= 1000.0 && 1000.0 < b.hi_hz)
        .expect("a band holds 1 kHz");
    let loudest = s
        .bands
        .iter()
        .max_by(|a, b| a.db.total_cmp(&b.db))
        .expect("a loudest band");
    assert_eq!(loudest.lo_hz, holding.lo_hz, "and no other band is louder");
    for band in s.bands.iter().filter(|b| b.hi_hz <= 500.0) {
        assert!(band.db < holding.db - 40.0, "well below it: {band:?}");
    }
}
