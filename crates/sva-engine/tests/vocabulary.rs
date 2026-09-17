// Concern: proves every admitted name types as the family it belongs to | Non-concern: the numbers each produces (sva-formula) | IO: (a composition) -> Ty

mod fixtures;

use fixtures::graph_of;
use sva_engine::{BUILTINS, Held, Value, Var, recognized_named, types};
use sva_formula::Body;

fn ty_of(name: &str, body: &str) -> sva_engine::Ty {
    let g = graph_of(name, &[("node", body)]);
    let typing = types(&g, "node").unwrap_or_else(|e| panic!("{name}: {e}"));
    typing.ty(typing.id("node").expect("the root"))
}

/// The one number a constant body normalizes to.
fn folded(name: &str, body: &str) -> f64 {
    let g = graph_of(name, &[("node", body)]);
    let typing = types(&g, "node").unwrap_or_else(|e| panic!("{name}: {e}"));
    let Value::ClosedForm(form) = typing.value(typing.id("node").expect("the root")) else {
        panic!("{name}: a law");
    };
    let sum = sva_formula::normalize_closed_form(form).unwrap_or_else(|_| panic!("{name}: a sum"));
    let [lane] = sum.lanes.as_slice() else {
        panic!("{name}: one lane");
    };
    match lane.atoms.as_slice() {
        [atom] if atom.is_bare() => atom.c.re,
        other => panic!("{name}: expected one bare atom, got {other:?}"),
    }
}

/// A keyed constant reads no rate and no grid: one key and one seed give one number, and
/// the node's type carries no rate for an observation to disagree with.
#[test]
fn rand_is_rate_free() {
    let drawn = |name: &str| {
        let g = graph_of(name, &[("node", "rand(1, seed=7)\n")]);
        let typing = types(&g, "node").expect("a constant");
        let id = typing.id("node").expect("the root");
        assert_eq!(typing.ty(id).rate, None, "a constant has no rate");
        let Value::ClosedForm(form) = typing.value(id) else {
            panic!("a law");
        };
        match &form.body {
            Body::Const(c) => *c,
            other => panic!("expected one constant, got {other:?}"),
        }
    };
    let first = drawn("rand-a");
    let second = drawn("rand-b");
    assert_eq!(first, second, "one key and one seed give one number");
    assert!(
        first.re >= 0.0 && first.re <= 1.0,
        "the unit interval: {first:?}"
    );

    let other = graph_of("rand-c", &[("node", "rand(2, seed=7)\n")]);
    let typing = types(&other, "node").expect("a constant");
    let Value::ClosedForm(form) = typing.value(typing.id("node").expect("the root")) else {
        panic!("a law");
    };
    assert!(
        !matches!(form.body, Body::Const(c) if c == first),
        "another key is another number"
    );
}

#[test]
fn noise_has_a_dual() {
    assert!(
        ty_of("noise", "noise(3, period=0.25, color=-3)\n").has_dual(),
        "a noise realization is a line spectrum on the 1/period grid"
    );
}

/// The two physical families share no name, and no argument list crosses between them.
#[test]
fn asking_for_an_fd_builtin_never_runs_a_modal_one() {
    assert_eq!(
        ty_of("fd", "chaigne_askenfelt(261.63)\n").held,
        Held::Sampled
    );
    assert!(ty_of("modal", "string(261.63)\n").has_dual());
    for (fd, modal) in [
        ("chaigne_askenfelt", "string"),
        ("rhaouti_chaigne_joly", "membrane"),
        ("chaigne_doutaut", "bar"),
        ("darabundit_scavone", "bore"),
        ("botteldooren", "room"),
    ] {
        assert_ne!(fd, modal);
        assert!(BUILTINS.contains(&fd), "{fd} is callable");
        assert!(BUILTINS.contains(&modal), "{modal} is callable");
    }
}

#[test]
fn every_modal_instrument_and_the_two_window_names_are_pairs() {
    for (name, body) in [
        ("string", "string(261.63, vel=3.2)\n"),
        ("membrane", "membrane(0.28, ly=0.36)\n"),
        ("bar", "bar(0.35)\n"),
        ("bore", "bore(0.6, closed=1)\n"),
        ("room", "room(5.0, ly=4.0, lz=2.7)\n"),
        ("hammer_pulse", "hammer_pulse(3.2)\n"),
        ("helmholtz", "helmholtz(0.002)\n"),
        (
            "a shouldered crop",
            "crop(sin(2*pi*220*t), 0s, 1s, rise=0.01, fall=0.05)\n",
        ),
    ] {
        assert!(ty_of(name, body).has_dual(), "{name}");
    }
}

/// A seed names one realization, so it is one number; a key is read wherever it points.
#[test]
fn a_seed_that_is_not_a_number_refuses() {
    let g = graph_of("key", &[("node", "noise(t)\n")]);
    assert!(types(&g, "node").is_err(), "a law is not a seed");
}

/// Skipping a positional would slide every argument after it into the wrong field.
#[test]
fn a_geometry_argument_that_is_not_a_number_refuses() {
    let g = graph_of("sliding", &[("node", "room(t, 4.0, 2.7)\n")]);
    assert!(types(&g, "node").is_err(), "a law is not a room's width");
}

/// A real exponent over a positive constant base is that number, never a truncated power.
#[test]
fn a_non_integer_constant_power_folds_exactly() {
    assert_eq!(
        folded("root-of-two", "pow(2, 1.5)\n"),
        2f64.powf(1.5),
        "pow(2, 1.5) is not 2"
    );
    assert_eq!(
        ty_of("real-exponent-of-a-signal", "pow(2, t)\n").held,
        Held::Form(Var::T),
        "a base raised to a moving exponent is exp(t*ln 2), which grows"
    );
}

/// `st` and a written power of two are the same transposition.
#[test]
fn pow_two_to_n_over_twelve_transposes_exactly() {
    assert_eq!(
        folded("written-power", "440*pow(2, 7/12)\n"),
        folded("semitones", "440*7st\n"),
        "a fifth is a fifth however it is written"
    );
}

/// A key that quantizes `t` is closed in `t` and takes no transform, so it keeps no dual.
#[test]
fn a_quantized_time_key_is_a_closed_form_in_t_with_no_dual() {
    let quantized = ty_of("held", "rand(t - t % 0.0625, seed=17)\n");
    assert_eq!(
        (quantized.held, quantized.dual),
        (Held::Form(Var::T), false)
    );
    assert!(
        ty_of("drawn-once", "rand(3, seed=17)\n").has_dual(),
        "a constant key is one number"
    );

    let form = |name: &str, files: &[(&str, &str)]| {
        let g = graph_of(name, files);
        let typing = types(&g, "node").unwrap_or_else(|e| panic!("{name}: {e}"));
        let ty = typing.ty(typing.id("node").expect("the root"));
        (ty.held, ty.dual)
    };
    assert_eq!(
        form(
            "keyed-on-a-law",
            &[("clock", "sin(t)\n"), ("node", "rand(@clock, seed=1)\n")]
        ),
        (Held::Form(Var::T), false),
        "a ref to a closed form is a key that moves"
    );
    assert_eq!(
        form(
            "keyed-on-a-number",
            &[("pitch", "440\n"), ("node", "rand(@pitch, seed=1)\n")]
        ),
        (Held::Form(Var::T), true),
        "a ref to one number is one key"
    );
}

/// `builtins` prints the name list; `lower/` decides what the engine lowers. Deriving the
/// first from the second is what keeps a printed name from being a refusal on call.
#[test]
fn the_vocabulary_names_exactly_what_the_engine_lowers() {
    for name in sva_engine::overload::FINITE_DIFFERENCE {
        assert!(
            BUILTINS.contains(&name),
            "`{name}` lowers to a solver but is not callable"
        );
    }
    let mut held: Vec<&str> = BUILTINS.to_vec();
    held.sort_unstable();
    let mut once = held.clone();
    once.dedup();
    assert_eq!(held.len(), once.len(), "one name, one entry: {held:?}");
}

/// One written `vel=` reaches whichever geometry it is written on.
#[test]
fn every_modal_bank_names_the_shared_damping_and_excitation() {
    for bank in ["string", "membrane", "bar", "bore", "room", "helmholtz"] {
        let named = recognized_named(bank).unwrap_or_else(|| panic!("{bank} is a builtin"));
        for shared in ["damp_dc", "damp_freq", "vel", "at", "contact", "p", "force"] {
            assert!(named.contains(&shared), "{bank} names {shared}: {named:?}");
        }
        assert_eq!(
            named.contains(&"modes"),
            bank != "helmholtz",
            "{bank}: only a cavity, at one mode, has no budget to name"
        );
    }
}
