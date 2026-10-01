// Concern: decides which names the language owns: reserved words, notes, builtins | Non-concern: what any of them evaluates to, a builtin's named arguments | IO: (name) -> bool

use crate::expr::{INDEX, JOIN, SERIES};

pub const SELF: &str = "self";

/// Every name the language reserves; `self` is call-only, the rest are values.
pub const RESERVED: [(&str, &str); 6] = [
    ("t", "time in seconds"),
    ("f", "frequency in hertz"),
    ("i", "the imaginary unit"),
    ("pi", "the constant pi"),
    (
        "inf",
        "infinity, a value: a crop edge never reached, a sum's open bound; arithmetic that \
         leaves no number refuses",
    ),
    (
        SELF,
        "a loop's own past: self(t - 17ms) in a continuous loop, self[idx(t) - 1] in a discrete one",
    ),
];

pub const CHANNEL: &str = "ch";

const ARITHMETIC: [&str; 22] = [
    "sin", "cos", "exp", "log", "pow", "sqrt", "abs", "tanh", "step", "max", "min", "saw",
    "square", "triangle", "sat", "crop", "rand", "delta", "pv", SERIES, JOIN, CHANNEL,
];

pub const CASTS: [&str; 5] = ["sample", "fourier", "ifourier", "stft", "istft"];

pub const FINITE_DIFFERENCE: [&str; 6] = [
    "chaigne_askenfelt",
    "willemsen_bilbao_serafin",
    "darabundit_scavone",
    "rhaouti_chaigne_joly",
    "chaigne_doutaut",
    "botteldooren",
];

pub const MODAL: [&str; 7] = [
    "string",
    "membrane",
    "bar",
    "bore",
    "room",
    "hammer_pulse",
    "helmholtz",
];

/// In the order sva-formula's `ALL_SHAPES` lists their maths.
pub const FILTERS: [&str; 8] = [
    "lp",
    "lowpass",
    "highpass",
    "bandpass",
    "notch",
    "peaking",
    "lowshelf",
    "highshelf",
];

const WRITTEN_LAWS: [&str; 1] = ["noise"];

const LAWS: [&str; WRITTEN_LAWS.len() + MODAL.len()] = {
    let mut all = [""; WRITTEN_LAWS.len() + MODAL.len()];
    let mut i = 0;
    while i < WRITTEN_LAWS.len() {
        all[i] = WRITTEN_LAWS[i];
        i += 1;
    }
    let mut j = 0;
    while j < MODAL.len() {
        all[WRITTEN_LAWS.len() + j] = MODAL[j];
        j += 1;
    }
    all
};

pub const BUILTINS: [&str; ARITHMETIC.len() + FINITE_DIFFERENCE.len() + CASTS.len() + LAWS.len()] = {
    let mut all = [""; ARITHMETIC.len() + FINITE_DIFFERENCE.len() + CASTS.len() + LAWS.len()];
    let mut i = 0;
    while i < ARITHMETIC.len() {
        all[i] = ARITHMETIC[i];
        i += 1;
    }
    let mut j = 0;
    while j < FINITE_DIFFERENCE.len() {
        all[ARITHMETIC.len() + j] = FINITE_DIFFERENCE[j];
        j += 1;
    }
    let mut k = 0;
    while k < CASTS.len() {
        all[ARITHMETIC.len() + FINITE_DIFFERENCE.len() + k] = CASTS[k];
        k += 1;
    }
    let mut n = 0;
    while n < LAWS.len() {
        all[ARITHMETIC.len() + FINITE_DIFFERENCE.len() + CASTS.len() + n] = LAWS[n];
        n += 1;
    }
    all
};

pub fn is_builtin(name: &str) -> bool {
    FILTERS.contains(&name) || BUILTINS.contains(&name) || name == INDEX
}

/// `#` is in no character class, so an accidental is spelled `Cs4`/`Db4`.
pub fn note_midi(name: &str) -> Option<i32> {
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
    let octave = octave.parse::<u8>().ok()?;
    let midi = (i32::from(octave) + 1) * 12 + step + accidental;
    (0..=127).contains(&midi).then_some(midi)
}

pub fn is_language_value(name: &str) -> bool {
    let reserved = RESERVED.iter().any(|(held, _)| *held == name);
    (reserved && name != SELF) || note_midi(name).is_some()
}

pub fn is_reserved(name: &str) -> bool {
    is_language_value(name) || name == SELF || is_builtin(name)
}
