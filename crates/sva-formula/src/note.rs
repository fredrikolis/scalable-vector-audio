// Concern: resolves a note-name identifier to its frequency | Non-concern: naming a measured peak (sva-samples), which names are bound (sva-engine) | IO: (&str) -> Option<Hz>

/// `A4` = 440 Hz = MIDI 69, twelve-tone equal temperament. `#` is in no character class, so
/// an accidental is spelled `Cs4`/`Db4`. Names run out where MIDI does, at `G9`.
pub fn frequency(name: &str) -> Option<f64> {
    Some(440.0 * ((f64::from(midi(name)?) - 69.0) / 12.0).exp2())
}

fn midi(name: &str) -> Option<i32> {
    let letter = name.chars().next()?;
    let step = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = &name[letter.len_utf8()..];
    let (accidental, octave) = match rest.as_bytes().first() {
        Some(b's') => (1, &rest[1..]),
        Some(b'b') => (-1, &rest[1..]),
        _ => (0, rest),
    };
    if octave.is_empty() || !octave.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // `u8` holds every octave in range and overflows no arithmetic below.
    let octave = octave.parse::<u8>().ok()?;
    let midi = (i32::from(octave) + 1) * 12 + step + accidental;
    (0..=127).contains(&midi).then_some(midi)
}
