// Concern: proves which names the language answers itself, notes and builtins included | Non-concern: what any of them evaluates to (sva-engine) | IO: (name) -> bool or a MIDI number

use sva_ast::{FILTERS, RESERVED, is_builtin, is_language_value, is_reserved, note_midi};

#[test]
fn the_tuning_anchor_is_midi_69() {
    assert_eq!(note_midi("A4"), Some(69));
    assert_eq!(note_midi("C4"), Some(60));
    assert_eq!(note_midi("As4"), Some(70));
}

#[test]
fn accidentals_are_spelled_with_letters_and_meet_in_equal_temperament() {
    assert_eq!(note_midi("Cs4"), note_midi("Db4"));
    assert_eq!(note_midi("Bs3"), note_midi("C4"), "B sharp is the next C");
    assert_eq!(note_midi("Fb4"), note_midi("E4"));
}

/// The octave is parsed off a user's string, so the name has to run out where MIDI does
/// instead of wrapping its arithmetic into some other note.
#[test]
fn an_octave_beyond_midi_is_no_note() {
    assert_eq!(note_midi("G9"), Some(127), "MIDI 127 is still a note");
    assert_eq!(note_midi("As9"), None, "MIDI 130 is past the range");
    assert_eq!(note_midi("C10"), None);
    assert_eq!(
        note_midi("C178956971"),
        None,
        "an octave that would overflow"
    );
}

#[test]
fn anything_that_is_not_a_note_is_none_rather_than_a_guess() {
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
        assert_eq!(note_midi(not_a_note), None, "{not_a_note}");
    }
}

#[test]
fn every_reserved_name_is_reserved_and_all_but_self_read_as_values() {
    for (name, _) in RESERVED {
        assert!(is_reserved(name), "{name}");
        assert_eq!(is_language_value(name), name != "self", "{name}");
    }
    assert!(is_language_value("A4") && is_reserved("Db2"));
    assert!(!is_reserved("cutoff") && !is_language_value("f0"));
}

#[test]
fn builtins_include_the_filters_the_series_and_the_sample_index() {
    for name in FILTERS.into_iter().chain([
        "sin",
        "sum",
        "idx",
        "sample",
        "noise",
        "string",
        "botteldooren",
        "ch",
        "join",
        "crop",
    ]) {
        assert!(is_builtin(name), "{name}");
        assert!(is_reserved(name), "{name}");
    }
    for name in ["x", "f0", "self", "t", "A4", "comb"] {
        assert!(!is_builtin(name), "{name}");
    }
}
