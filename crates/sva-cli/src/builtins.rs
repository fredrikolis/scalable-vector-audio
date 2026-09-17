// Concern: assembles the language's callable/syntactic vocabulary off the engine's own tables | Non-concern: what a builtin renders, the JSON shape (output.rs) | IO: none -> a Builtins value

use sva_engine::overload::{notation, signature};
use sva_engine::{BUILTINS, Cast, Codomain, Held, MAX_WIDTH, REGISTRY, Ty, Var, recognized_named};
use sva_formula::filter::{ALL_SHAPES, Shape};
use sva_formula::{FAMILIES, TABLE_VERSION};

pub struct Callable {
    pub name: &'static str,
    pub required: usize,
    pub max_positional: usize,
    pub named: &'static [&'static str],
    /// A subset of `named` with no default — a caller who omits one gets a refusal, not a
    /// fallback. Every builtin leaves this empty today.
    pub required_named: &'static [&'static str],
    pub takes_gain: Option<bool>,
}

/// `(spelling, meaning)`, in the lexer's own longest-match-first order.
pub const UNIT_SUFFIXES: [(&str, &str); 11] = [
    ("khz", "kilohertz (x1000)"),
    ("ms", "milliseconds (seconds x0.001)"),
    ("sp", "samples"),
    ("hz", "hertz"),
    ("db", "decibels (log)"),
    ("ct", "cents (log)"),
    ("st", "semitones (log)"),
    ("b", "bars, resolved against bpm/meter"),
    ("s", "seconds"),
    ("m", "minutes (seconds x60)"),
    ("h", "hours (seconds x3600)"),
];

pub const NOTE_GRAMMAR: &str = "a bare identifier: a letter A-G, an optional accidental spelled \
    s (sharp) or b (flat) -- never #, then a whole-number octave, e.g. A4, Cs4, Db4; A4 = 440 Hz, \
    twelve-tone equal temperament, up to G9 where MIDI's 128 notes end";

/// `(name, what it resolves to)`.
pub const RESERVED: [(&str, &str); 5] = [
    ("t", "time in seconds"),
    ("f", "frequency in hertz"),
    ("i", "the imaginary unit"),
    ("pi", "the constant pi"),
    ("self", "a bounded self-reference, call-only: self(t - 1sp)"),
];

/// `(name, its call shape)`.
pub const SPECIAL_FORMS: [(&str, &str); 5] = [
    (
        "sum",
        "sum(index, lo, hi, expr) -- index is a name the series binds, not a value; hi may be \
         inf, which makes it a series",
    ),
    (
        "crop",
        "crop(x, start, end) -- windows x to [start, end) seconds, zero outside",
    ),
    (
        "join",
        "join(a, b, ...) -- builds one wide value from 2..=8 mono args",
    ),
    (
        "ch",
        "ch(x, index) -- extracts one component of a wide value by a literal index",
    ),
    (
        "noise",
        "noise(seed, period=, color=) -- one line per 1/period hertz, each of unit \
         amplitude, so the series' own RMS is the square root of half its line count; scale \
         it to the level the piece wants",
    ),
];

/// Confirmed against the lexer's and parser's own tables, not asserted from memory: no token
/// exists for any of these, so none can appear in an expression at all.
pub const NOT_SUPPORTED: [&str; 4] = [
    "comparison operators: < > <= >= == !=",
    "boolean operators: && || !",
    "conditionals: if/else, a ternary",
    "exponentiation `^`: write pow(base, exponent)",
];

/// One written cast, and every representation it takes to which.
pub struct Crossing {
    pub name: &'static str,
    pub rows: Vec<(&'static str, &'static str)>,
}

pub struct Builtins {
    pub callables: Vec<Callable>,
    pub casts: Vec<Crossing>,
    pub table_version: u64,
    pub families: &'static [(&'static str, &'static str)],
    pub refusals: &'static [(&'static str, &'static str)],
    pub unit_suffixes: &'static [(&'static str, &'static str)],
    pub note_names: &'static str,
    pub reserved: &'static [(&'static str, &'static str)],
    pub special_forms: &'static [(&'static str, &'static str)],
    pub not_supported: &'static [&'static str],
}

fn plain_callable(name: &'static str) -> Callable {
    let sig = signature(name)
        .unwrap_or_else(|| unreachable!("{name} is in BUILTINS but SIGNATURES does not cover it"));
    Callable {
        name,
        required: sig.required(),
        max_positional: if sig.variadic {
            MAX_WIDTH
        } else {
            sig.params.len()
        },
        named: sig.named(),
        required_named: &[],
        takes_gain: None,
    }
}

/// `q` is refused outright on `Shape::OnePole` rather than merely unused, so its own positional
/// ceiling sits below every other shape's.
fn filter_callable(shape: Shape) -> Callable {
    let gain = shape.takes_gain();
    let max_positional = if shape == Shape::OnePole {
        2
    } else if gain {
        4
    } else {
        3
    };
    Callable {
        name: shape.name(),
        required: 2,
        max_positional,
        named: recognized_named(shape.name()).unwrap_or(&[]),
        required_named: &[],
        takes_gain: Some(gain),
    }
}

/// Every state a value's form can be in, so a crossing table shows which duals a cast needs.
const HELD: [(Held, bool); 6] = [
    (Held::Form(Var::T), false),
    (Held::Form(Var::T), true),
    (Held::Form(Var::F), false),
    (Held::Form(Var::F), true),
    (Held::Sampled, false),
    (Held::Frames, false),
];

/// Read off `Cast::resolve` rather than restated: a row exists where the cast answers.
fn crossings() -> Vec<Crossing> {
    Cast::NAMES
        .into_iter()
        .map(|name| {
            let cast = Cast::from_name(name, &[("window", 1024.0), ("hop", 256.0)])
                .unwrap_or_else(|| unreachable!("{name} is one of Cast::NAMES"));
            Crossing {
                name,
                rows: HELD
                    .into_iter()
                    .filter_map(|(held, dual)| {
                        let ty = Ty {
                            dual,
                            ..Ty::discrete(held, Codomain::Real)
                        };
                        cast.resolve(&[ty])
                            .ok()
                            .map(|out| (notation(ty), notation(out)))
                    })
                    .collect(),
            }
        })
        .collect()
}

pub fn builtins() -> Builtins {
    let mut callables: Vec<Callable> = BUILTINS.into_iter().map(plain_callable).collect();
    callables.extend(ALL_SHAPES.into_iter().map(filter_callable));
    Builtins {
        callables,
        casts: crossings(),
        table_version: TABLE_VERSION,
        families: &FAMILIES,
        refusals: &REGISTRY,
        unit_suffixes: &UNIT_SUFFIXES,
        note_names: NOTE_GRAMMAR,
        reserved: &RESERVED,
        special_forms: &SPECIAL_FORMS,
        not_supported: &NOT_SUPPORTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_note_grammar_this_dump_states_is_what_note_actually_resolves() {
        for note in ["A4", "Cs4", "Db4"] {
            assert!(
                sva_formula::note::frequency(note).is_some(),
                "{note} should be a note"
            );
        }
        for not_a_note in ["H4", "Cs", "C99999999999"] {
            assert!(
                sva_formula::note::frequency(not_a_note).is_none(),
                "{not_a_note} is not the grammar this dump states"
            );
        }
    }

    #[test]
    fn every_operator_this_dump_says_does_not_exist_actually_refuses_to_parse() {
        for bad in [
            "1 < 2",
            "1 > 2",
            "1 <= 2",
            "1 >= 2",
            "1 == 2",
            "1 != 2",
            "1 && 1",
            "1 || 1",
            "!1",
            "1 ? 2 : 3",
            "2 ^ 3",
        ] {
            assert!(
                sva_ast::parse_expr(bad).is_err(),
                "`{bad}` parsed, so this dump's not_supported claim is stale"
            );
        }
    }

    #[test]
    fn every_builtin_name_gets_exactly_one_callable_entry() {
        let b = builtins();
        assert_eq!(b.callables.len(), BUILTINS.len() + ALL_SHAPES.len());
        for name in BUILTINS {
            assert_eq!(
                b.callables.iter().filter(|c| c.name == name).count(),
                1,
                "{name} should appear exactly once"
            );
        }
        for shape in ALL_SHAPES {
            let found = b
                .callables
                .iter()
                .find(|c| c.name == shape.name())
                .unwrap_or_else(|| panic!("{} is missing", shape.name()));
            assert_eq!(found.takes_gain, Some(shape.takes_gain()));
        }
    }

    #[test]
    fn one_pole_alone_has_no_room_for_q_or_gain() {
        let b = builtins();
        let lp = b.callables.iter().find(|c| c.name == "lp").unwrap();
        assert_eq!(lp.max_positional, 2);
        let lowpass = b.callables.iter().find(|c| c.name == "lowpass").unwrap();
        assert_eq!(lowpass.max_positional, 3);
        let peaking = b.callables.iter().find(|c| c.name == "peaking").unwrap();
        assert_eq!(peaking.max_positional, 4);
    }

    #[test]
    fn each_reserved_name_still_behaves_the_way_this_dump_says_it_does() {
        for (name, _) in RESERVED {
            assert!(
                sva_engine::instantiate::is_reserved(name),
                "`{name}` is listed as reserved but a parameter may still take it"
            );
            if name == "self" {
                continue;
            }
            assert!(
                sva_ast::parse_expr(name).is_ok(),
                "`{name}` should parse as the special name this dump claims"
            );
        }
        assert!(
            !sva_engine::instantiate::is_reserved("cutoff"),
            "an ordinary parameter name must not be reserved, or the claim says nothing"
        );
        assert!(
            sva_ast::parse_expr("self").is_err(),
            "`self` alone must still require a call form"
        );
        assert!(
            sva_ast::parse_expr("self(t - 1sp)").is_ok(),
            "`self(...)` must still parse as a bounded self-reference"
        );
    }
}
