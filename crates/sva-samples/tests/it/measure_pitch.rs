// Concern: states what naming a peak owes a tone and its partials | Non-concern: finding the peaks (measure_spectrum.rs) | IO: (peaks or samples) -> asserted notes

use sva_samples::measure::pitch::{name_peaks, track};
use sva_samples::measure::spectrum::Peak;

fn peak(hz: f64, db: f64) -> Peak {
    Peak { hz, db }
}

fn sine(hz: f64, rate: f64, secs: f64) -> Vec<f64> {
    let n = (rate * secs) as usize;
    (0..n)
        .map(|k| (std::f64::consts::TAU * hz * k as f64 / rate).sin())
        .collect()
}

#[test]
fn a_peak_is_named_by_the_equal_tempered_note_nearest_it() {
    let named = name_peaks(&[peak(440.0, -6.0), peak(261.6255653, -9.0)], 8);
    assert_eq!(named[0].name, "A4", "440 Hz is A4");
    assert_eq!(named[1].name, "C4", "261.63 Hz is middle C");
    for note in &named {
        assert!(note.cents.abs() < 0.01, "{note:?} sits on the note");
    }
}

/// The cents field is what says a peak is not the note it was named by.
#[test]
fn a_peak_between_two_notes_is_named_by_one_and_detuned_from_it() {
    let sharp = 440.0 * 2f64.powf(0.25 / 12.0);
    let held = name_peaks(&[peak(sharp, -6.0)], 8);
    let [named] = held.as_slice() else {
        unreachable!("one peak in, one note out")
    };
    assert_eq!(named.name, "A4", "still nearest A4");
    assert!(
        (named.cents - 25.0).abs() < 1e-6,
        "a quarter of a semitone sharp: {named:?}"
    );
}

/// A saw's partials must not read as a chord of their own.
#[test]
fn a_partial_over_a_louder_peak_is_reported_as_its_harmonic() {
    let named = name_peaks(
        &[peak(220.0, -3.0), peak(440.0, -9.0), peak(660.0, -12.0)],
        8,
    );
    assert_eq!(
        named[0].harmonic_of, None,
        "the loudest is nobody's partial"
    );
    assert_eq!(named[1].harmonic_of, Some(220.0), "the octave");
    assert_eq!(named[2].harmonic_of, Some(220.0), "and the twelfth");
}

/// A fifth is 1.4983 times its root, which is no near-integer multiple of it.
#[test]
fn a_note_that_is_no_whole_multiple_of_a_louder_one_stands_on_its_own() {
    let named = name_peaks(&[peak(220.0, -3.0), peak(329.6275569, -9.0)], 8);
    assert_eq!(named[1].harmonic_of, None, "{:?} is a fifth", named[1]);
}

/// Which note heads a series is decided by the notes, not by the order they arrive in.
#[test]
fn which_note_heads_a_series_does_not_depend_on_the_order_peaks_arrive_in() {
    let named = name_peaks(&[peak(440.0, -9.0), peak(220.0, -3.0)], 8);
    assert_eq!(named[0].harmonic_of, Some(220.0), "{named:?}");
    assert_eq!(named[1].harmonic_of, None, "{named:?}");
}

/// A breathy voice's fundamental is often weaker than its octave, which must not read as
/// the pitch.
#[test]
fn a_fundamental_weaker_than_its_octave_still_heads_its_series() {
    let named = name_peaks(
        &[
            peak(440.0, -6.0),
            peak(660.0, -12.0),
            peak(880.0, -14.0),
            peak(220.0, -18.0),
        ],
        8,
    );
    let heads: Vec<f64> = named
        .iter()
        .filter(|n| n.harmonic_of.is_none())
        .map(|n| n.hz)
        .collect();
    assert_eq!(heads, [220.0], "one series under 220 Hz: {named:?}");
}

/// The subharmonic sum weights each higher multiple less, so a faint note an octave under a
/// strong one explains it worse than the strong one explains itself.
#[test]
fn a_faint_note_an_octave_down_does_not_claim_a_strong_one() {
    let named = name_peaks(&[peak(440.0, -3.0), peak(220.0, -40.0)], 8);
    assert!(
        named.iter().all(|n| n.harmonic_of.is_none()),
        "two notes, neither the other's partial: {named:?}"
    );
}

#[test]
fn a_peak_outside_the_named_band_is_dropped_rather_than_named() {
    let named = name_peaks(
        &[peak(20.0, -3.0), peak(6000.0, -4.0), peak(440.0, -6.0)],
        8,
    );
    assert_eq!(named.len(), 1, "one of the three is in band: {named:?}");
    assert_eq!(named[0].name, "A4");
}

#[test]
fn max_notes_bounds_what_one_frame_names() {
    let found = [
        peak(220.0, -3.0),
        peak(330.0, -6.0),
        peak(440.0, -9.0),
        peak(550.0, -12.0),
    ];
    assert_eq!(name_peaks(&found, 2).len(), 2, "the two loudest");
    assert_eq!(name_peaks(&found, 0).len(), 0, "or none at all");
}

#[test]
fn a_tone_is_tracked_frame_by_frame_from_the_instant_it_was_read_at() {
    let rate = 44100.0;
    let frames = track(&sine(440.0, rate, 0.4), rate, 2.0, 0.1, 4);
    assert_eq!(frames.len(), 4, "0.4 s in 0.1 s frames: {}", frames.len());
    for (n, frame) in frames.iter().enumerate() {
        assert!(
            (frame.t_secs - (2.0 + n as f64 * 0.1)).abs() < 1e-3,
            "frame {n} starts at {}",
            frame.t_secs
        );
        assert_eq!(
            frame.notes.first().map(|note| note.name.as_str()),
            Some("A4"),
            "the loudest thing in a 440 Hz tone: {:?}",
            frame.notes
        );
    }
}

/// The same off samples: a tone whose fundamental sits 12 dB under its octave is tracked as
/// its fundamental in every frame, every partial over it named its harmonic.
#[test]
fn a_tone_with_a_weak_fundamental_is_tracked_as_its_fundamental() {
    let rate = 44100.0;
    let partials = [(220.0, 0.25), (440.0, 1.0), (660.0, 0.5), (880.0, 0.25)];
    let n = (rate * 0.4) as usize;
    let tone: Vec<f64> = (0..n)
        .map(|k| {
            let t = k as f64 / rate;
            partials
                .iter()
                .map(|(hz, a)| a * (std::f64::consts::TAU * hz * t).sin())
                .sum()
        })
        .collect();
    for frame in track(&tone, rate, 0.0, 0.1, 4) {
        let heads: Vec<&str> = frame
            .notes
            .iter()
            .filter(|n| n.harmonic_of.is_none())
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(heads, ["A3"], "220 Hz heads the tone: {:?}", frame.notes);
        assert_eq!(frame.notes.len(), 4, "{:?}", frame.notes);
    }
}

#[test]
fn silence_names_nothing() {
    let frames = track(&vec![0.0; 4410], 44100.0, 0.0, 0.1, 4);
    assert_eq!(frames.len(), 1);
    assert!(frames[0].notes.is_empty(), "{:?}", frames[0].notes);
}
