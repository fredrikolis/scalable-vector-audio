// Concern: a MIDI note number's frequency in twelve-tone equal temperament | Non-concern: the note-name grammar (sva-ast), naming a measured peak (sva-samples) | IO: (midi) -> Hz

/// `A4` = 440 Hz = MIDI 69.
pub fn frequency(midi: i32) -> f64 {
    440.0 * ((f64::from(midi) - 69.0) / 12.0).exp2()
}
