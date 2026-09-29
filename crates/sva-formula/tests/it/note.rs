// Concern: proves a note name resolves to the frequency the format states | Non-concern: naming a measured peak (sva-samples) | IO: (&str) -> Option<Hz>

use sva_formula::note::frequency;

#[test]
fn the_tuning_anchor_and_its_equal_tempered_neighbours() {
    assert_eq!(frequency("A4"), Some(440.0));
    assert_eq!(frequency("A5"), Some(880.0), "an octave is a doubling");
    assert!((frequency("C4").unwrap() - 261.6255653005986).abs() < 1e-9);
    assert!((frequency("As4").unwrap() / 440.0 - 2f64.powf(1.0 / 12.0)).abs() < 1e-12);
}

#[test]
fn accidentals_are_spelled_with_letters_and_meet_in_equal_temperament() {
    assert_eq!(frequency("Cs4"), frequency("Db4"));
    assert_eq!(frequency("Bs3"), frequency("C4"), "B sharp is the next C");
    assert_eq!(frequency("Fb4"), frequency("E4"));
}

/// The octave is parsed off a user's string, so the name has to run out where MIDI does
/// instead of wrapping its arithmetic into some other note's frequency.
#[test]
fn an_octave_beyond_midi_refuses() {
    assert!(frequency("G9").is_some(), "MIDI 127 is still a note");
    assert_eq!(frequency("As9"), None, "MIDI 130 is past the range");
    assert_eq!(frequency("C10"), None);
    assert_eq!(
        frequency("C178956971"),
        None,
        "an octave that would overflow"
    );
}

#[test]
fn anything_that_is_not_a_note_resolves_to_nothing_rather_than_a_guess() {
    for not_a_note in [
        "H4",
        "C",
        "c4",
        "Cx4",
        "Cs",
        "pi",
        "",
        "C4x",
        "C99999999999",
    ] {
        assert_eq!(frequency(not_a_note), None, "{not_a_note}");
    }
}
