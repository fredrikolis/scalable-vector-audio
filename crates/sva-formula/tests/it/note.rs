// Concern: proves a MIDI note number resolves to the frequency the format states | Non-concern: the note-name grammar (sva-ast) | IO: (midi) -> Hz

use sva_formula::note::frequency;

#[test]
fn the_tuning_anchor_and_its_equal_tempered_neighbours() {
    assert_eq!(frequency(69), 440.0);
    assert_eq!(frequency(81), 880.0, "an octave is a doubling");
    assert!((frequency(60) - 261.6255653005986).abs() < 1e-9);
    assert!((frequency(70) / 440.0 - 2f64.powf(1.0 / 12.0)).abs() < 1e-12);
}
