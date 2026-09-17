// Concern: proves each crossing FORMAT 16 names refuses under its own code and offers the cast to write | Non-concern: what types when nothing crosses (typing.rs) | IO: (a composition) -> a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::{EngineError, Held, RenderConfig, Var, render, types};

fn refusal(name: &str, files: &[(&str, &str)], root: &str) -> EngineError {
    types(&graph_of(name, files), root).expect_err("a refusal")
}

fn parts(e: &EngineError) -> (String, String, String) {
    match e {
        EngineError::Refused(d) => (d.code.clone(), d.message.clone(), d.help.clone()),
        other => panic!("expected a written refusal, got {other:?}"),
    }
}

const CHORD: &str = "sin(2*pi*261.63*t) + sin(2*pi*329.63*t)\n";

#[test]
fn mixing_samples_and_a_law_names_sample() {
    let e = refusal(
        "samples-and-law",
        &[
            ("body", "chaigne_askenfelt(261.63)\n"),
            ("master", "@body*sin(2*pi*3*t)\n"),
        ],
        "master",
    );
    let (code, _, help) = parts(&e);
    assert_eq!(code, "type.samples_in_closed_form");
    assert!(
        help.contains("sample("),
        "the repair names the cast: {help}"
    );
}

#[test]
fn mixing_t_and_f_names_fourier() {
    let e = refusal(
        "t-and-f",
        &[
            ("carrier", "tanh(sin(2*pi*100*t)*3)\n"),
            ("mask", "1/(1 + pow(f/300, 8))\n"),
            ("master", "@carrier*@mask\n"),
        ],
        "master",
    );
    let (code, _, help) = parts(&e);
    assert_eq!(code, "type.domain_mismatch");
    assert!(
        help.contains("fourier"),
        "the repair names both casts: {help}"
    );
    assert!(help.contains("ifourier"), "{help}");
}

#[test]
fn a_pair_plus_a_t_law_needs_no_cast() {
    let typing = types(
        &graph_of(
            "pair-plus-ct",
            &[
                ("chord", CHORD),
                ("drive", "sin(2*pi*55*t)\n"),
                ("master", "@chord + tanh(@drive*4)\n"),
            ],
        ),
        "master",
    )
    .expect("a dual beside a closed form in t needs no cast");
    let root = typing.ty(typing.id("master").expect("the root"));
    assert_eq!(
        (root.held, root.dual),
        (Held::Form(Var::T), false),
        "a dual meeting a closed form in t with none keeps none"
    );
}

#[test]
fn fourier_on_a_nonlinearity_names_the_subterm_and_the_cast() {
    let e = refusal(
        "fourier-on-a-nonlinearity",
        &[
            ("drive", "sin(2*pi*55*t)\n"),
            ("bell", "fourier(tanh(@drive*4))\n"),
        ],
        "bell",
    );
    let (code, message, help) = parts(&e);
    assert_eq!(code, "cast.left_algebra");
    assert!(
        message.contains("left A"),
        "the blocking subterm is named: {message}"
    );
    assert!(
        help.contains("sample("),
        "the cast to write instead: {help}"
    );
}

#[test]
fn a_bar_literal_inside_an_f_law_refuses() {
    let e = refusal("bars-in-f", &[("mask", "exp(0 - 1b*f)\n")], "mask");
    let (code, message, _) = parts(&e);
    assert_eq!(code, "type.bars_in_frequency");
    assert!(message.contains("bar"), "{message}");
}

#[test]
fn a_stft_argument_that_is_not_samples_names_sample() {
    let e = refusal(
        "stft-on-a-law",
        &[
            ("chord", CHORD),
            ("frames", "stft(@chord, window=1024, hop=256)\n"),
        ],
        "frames",
    );
    let (code, _, help) = parts(&e);
    assert_eq!(code, "cast.stft_needs_samples");
    assert!(help.contains("sample("), "{help}");
}

/// The three parts of FORMAT 7.1, printed as one line: what was refused, where the subterm
/// was written, and the cast to write instead.
#[test]
fn a_cast_refusal_prints_its_three_parts() {
    let e = refusal(
        "printed",
        &[("bell", "fourier(tanh(sin(2*pi*55*t)*4))\n")],
        "bell",
    );
    let printed = e.to_string();
    assert!(printed.contains("bell"), "{printed}");
    assert!(printed.contains("left A"), "{printed}");
    assert!(printed.contains("sample("), "{printed}");
}

/// A name given twice is an ambiguity the caller must settle, not a precedence rule the
/// engine picks for them.
#[test]
fn an_argument_given_by_position_and_by_name_refuses() {
    let e = refusal(
        "twice",
        &[
            ("chord", CHORD),
            ("master", "lowpass(@chord, 800, cutoff=900)\n"),
        ],
        "master",
    );
    let (code, _, help) = parts(&e);
    assert_eq!(code, "grammar.arity");
    assert!(help.contains("not both"), "{help}");
}

/// A signal is what a builtin operates on, so it is written where the call puts it; a key
/// the builtin never reads is a typo, not a default.
#[test]
fn a_signal_given_by_name_and_a_key_no_builtin_reads_both_refuse() {
    let by_name = refusal(
        "signal-by-name",
        &[("chord", CHORD), ("master", "sat(x=@chord, drive=2)\n")],
        "master",
    );
    let (code, _, help) = parts(&by_name);
    assert_eq!(code, "grammar.arity");
    assert!(help.contains("by position"), "{help}");

    let unread = refusal(
        "unread-key",
        &[
            ("chord", CHORD),
            ("master", "lowpass(@chord, 800, bogus=6)\n"),
        ],
        "master",
    );
    let (code, _, help) = parts(&unread);
    assert_eq!(code, "grammar.unknown_named_argument");
    assert!(help.contains("bogus"), "{help}");
}

/// A window's edges are numbers: an expression that moves names no edge, and is refused
/// rather than folded to zero.
#[test]
fn a_window_bound_that_moves_refuses() {
    let e = refusal(
        "moving-bound",
        &[
            ("grid", "chaigne_askenfelt(261.63)\n"),
            ("master", "crop(sample(sin(2*pi*220*t)), @grid, 1s)\n"),
        ],
        "master",
    );
    let (code, message, _) = parts(&e);
    assert_eq!(code, "engine.non_constant_argument");
    assert!(message.contains('a'), "{message}");
}

/// Point sampling is FORMAT 9.1's no-dual row, so it rescues a nonlinearity written in `t`
/// and not one written in `f`, which still names the construct it composed nothing across.
#[test]
fn a_nonlinearity_over_a_spectrum_names_the_construct() {
    let g = graph_of(
        "nonlinearity-in-f",
        &[(
            "shaped",
            "tanh(lowpass(exp(0 - pow(f/300, 2)), 800, 0.7))\n",
        )],
    );
    let e = render(&g, "shaped", RenderConfig::seconds(8_000, 0.01), None)
        .err()
        .expect("a nonlinearity over a crossing in f composes nothing");
    let (code, message, help) = parts(&e);
    assert_eq!(code, "read.no_spectral_sum");
    assert!(message.contains("tanh"), "{message}");
    assert!(help.contains("sample"), "{help}");
}

/// The registry is what `sva-cli builtins` prints as the whole refusal vocabulary, so a code
/// written in any crate a refusal crosses and left out of it is a hole in that listing.
#[test]
fn every_code_the_engine_can_raise_is_in_the_registry() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut written: Vec<String> = Vec::new();
    for name in ["sva-engine", "sva-samples", "sva-formula"] {
        for path in sources(&crates.join(name).join("src")) {
            let text = std::fs::read_to_string(&path).expect("a source file");
            written.extend(codes_in(&text));
        }
    }
    written.sort();
    written.dedup();
    assert!(written.len() > 20, "the scan found {} codes", written.len());
    for code in &written {
        assert!(
            sva_engine::REGISTRY.iter().any(|(name, _)| name == code),
            "`{code}` is written in a crate a refusal crosses, and the registry never names it"
        );
    }
    let mut named: Vec<&str> = sva_engine::REGISTRY.iter().map(|(name, _)| *name).collect();
    let before = named.len();
    named.sort_unstable();
    named.dedup();
    assert_eq!(before, named.len(), "the registry names a code twice");
}

fn sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for held in std::fs::read_dir(dir)
        .expect("a source directory")
        .flatten()
    {
        let path = held.path();
        match path.is_dir() {
            true => out.extend(sources(&path)),
            false if path.extension().is_some_and(|e| e == "rs") => out.push(path),
            false => {}
        }
    }
    out
}

/// A quoted `<judgment>.<name>`, which is the one shape every code in this crate is written in.
fn codes_in(text: &str) -> Vec<String> {
    const PREFIXES: [&str; 8] = [
        "type.",
        "cast.",
        "collapse.",
        "samples.",
        "grammar.",
        "engine.",
        "read.",
        "ref.",
    ];
    let mut out = Vec::new();
    for quoted in text.split('"').skip(1).step_by(2) {
        let shaped = PREFIXES.iter().any(|p| quoted.starts_with(p))
            && quoted
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c == '.')
            && quoted.matches('.').count() == 1;
        if shaped {
            out.push(quoted.to_string());
        }
    }
    out
}

/// A root of a signal has no value the algebra carries and no sign the engine may assume,
/// so the exponent is named rather than truncated to an integer.
#[test]
fn a_non_integer_power_of_a_signal_refuses_by_name() {
    let e = refusal(
        "root-of-a-signal",
        &[("node", "pow(sin(2*pi*440*t), 0.5)\n")],
        "node",
    );
    let (code, message, help) = parts(&e);
    assert_eq!(code, "type.non_integer_power");
    assert!(message.contains("0.5"), "the exponent is named: {message}");
    assert!(help.contains("exp("), "the repair is written: {help}");
    let big = refusal(
        "power-past-the-index",
        &[("node", "pow(sin(2*pi*440*t), 4000000000)\n")],
        "node",
    );
    assert_eq!(parts(&big).0, "type.non_integer_power");
}

/// An exact reading answers off a spectral sum. A term that reaches none says which subterm
/// blocked it, under a code of the reading's own rather than the render's.
#[test]
fn an_exact_reading_on_a_ct_names_the_blocking_term() {
    let g = graph_of(
        "no-spectral-sum",
        &[
            ("tone", "sin(2*pi*440*t)\n"),
            ("turned", "ifourier(fourier(@tone))\n"),
            ("master", "tanh(@turned(t))\n"),
        ],
    );
    let held = render(&g, "master", RenderConfig::seconds(8_000, 0.01), None)
        .expect("a continuous-time law still renders");
    let id = held.id("master").expect("the root");
    let e = sva_engine::answer(&held, id, sva_engine::Representation::Lines)
        .expect_err("a line list off a term no atom sum reaches");
    let (code, message, help) = parts(&e);
    assert_eq!(code, "read.no_spectral_sum");
    assert!(message.contains("tanh"), "the blocking subterm: {message}");
    assert!(message.contains("master"), "the term read: {message}");
    assert!(help.contains("sample("), "{help}");
}

/// A shoulder of no length is a hard edge, so `fall=0s` is a legal window and must render as
/// one. A shoulder outside the window it belongs to is not, and
/// names itself rather than aborting the process on a non-finite amplitude.
#[test]
fn a_zero_fall_shoulder_is_a_hard_edge_not_a_panic() {
    let rendered = |body: &str| {
        let g = graph_of("crop-edges", &[("node", body)]);
        render(&g, "node", RenderConfig::seconds(8_000, 4.0), None)
    };
    let rms = |body: &str| {
        let held = rendered(body).unwrap_or_else(|e| panic!("{body}: {e}"));
        let plane = held
            .buffer(held.id("node").expect("the root"))
            .expect("a buffer")
            .plane(0)
            .to_vec();
        (plane.iter().map(|s| s * s).sum::<f64>() / plane.len() as f64).sqrt()
    };

    let cropped = rms("crop(sin(2*pi*440*t), 0s, 4s)\n");
    assert!(cropped > 0.0);
    assert!(
        (rms("crop(sin(2*pi*440*t), 0s, 4s, rise=0s, fall=0s)\n") - cropped).abs() < 1e-12,
        "two hard edges are the crop itself"
    );
    let one_sided = rms("crop(sin(2*pi*440*t), 0s, 4s, rise=1s, fall=0s)\n");
    assert!(
        one_sided > 0.0 && one_sided < cropped,
        "a rise with no fall takes energy off the front alone: {one_sided}"
    );

    for body in [
        "crop(sin(2*pi*440*t), 0s, 4s, rise=-1s, fall=1s)\n",
        "crop(sin(2*pi*440*t), 0s, 4s, rise=3s, fall=3s)\n",
    ] {
        let Err(refused) = rendered(body) else {
            panic!("{body}: a shoulder outside its own window should refuse");
        };
        assert_eq!(refused.code(), "engine.bad_crop_shoulder", "{body}");
    }
}

/// The operand a cast cannot take is named by its form and axis, not by a type abbreviation.
#[test]
fn a_closed_form_without_a_dual_refuses_fourier_by_name() {
    let e = refusal(
        "fourier-without-a-dual",
        &[("rise", "pow(2, t)\n"), ("bell", "fourier(@rise)\n")],
        "bell",
    );
    let (code, message, _) = parts(&e);
    assert_eq!(code, "cast.left_algebra");
    assert!(
        message.contains("a closed form in `t` with no dual"),
        "the operand is named by its form and axis: {message}"
    );
}

/// A closed form with no dual gives a filter nothing to multiply against.
#[test]
fn filter_on_a_closed_form_without_a_dual_names_the_dual() {
    let e = refusal(
        "filter-without-a-dual",
        &[
            ("rise", "pow(2, t)\n"),
            ("bell", "lowpass(@rise, 800, 0.7)\n"),
        ],
        "bell",
    );
    let (code, message, help) = parts(&e);
    assert_eq!(code, "type.filter_needs_a_dual");
    assert!(
        message.contains("a closed form in `t` with no dual"),
        "the operand is named by its form and axis: {message}"
    );
    assert!(
        help.contains("sample("),
        "the repair names the cast: {help}"
    );
}

/// A solver's grid is sized from its arguments, so an out-of-range value must not reach one.
#[test]
fn a_finite_difference_argument_outside_its_range_refuses_before_a_grid_is_built() {
    let e = refusal(
        "physics-out-of-range",
        &[("node", "botteldooren(0)\n")],
        "node",
    );
    assert_eq!(e.code(), "engine.physics_out_of_range");
    assert!(e.to_string().contains("botteldooren"), "{e}");
}

/// A node count only the rate decides, so the grid checks it and a render carries it out.
#[test]
fn a_room_too_large_for_one_grid_refuses_with_the_ceiling_it_passed() {
    let graph = graph_of("room-too-large", &[("node", "botteldooren(40)\n")]);
    let config = sva_engine::RenderConfig::seconds(16_000, 0.01);
    let Err(refused) = sva_engine::render(&graph, "node", config, None) else {
        panic!("a 40 Hz room is millions of nodes");
    };
    let (code, message, _) = parts(&refused);
    assert_eq!(code, "samples.grid_too_large");
    assert!(message.contains("botteldooren"), "{message}");
    assert!(
        sva_engine::REGISTRY.iter().any(|(name, _)| *name == code),
        "`{code}` is in the vocabulary `builtins` prints"
    );
}

/// A node whose whole content is text is a reserved variable, a meter or a key. Only a note name
/// reads as a frequency, and the rest is no call for the builtin table to be missing.
#[test]
fn a_text_node_that_names_no_note_says_so() {
    let e = refusal("text-node", &[("node", "4/4\n")], "node");
    let (code, message, help) = parts(&e);
    assert_eq!(code, "grammar.unknown_name");
    assert!(message.contains("4/4"), "{message}");
    assert!(!message.contains("function"), "it is no call: {message}");
    assert!(help.contains("note name"), "{help}");
    assert!(
        sva_engine::REGISTRY.iter().any(|(name, _)| *name == code),
        "`{code}` is in the vocabulary `builtins` prints"
    );

    let named = sva_engine::types(&graph_of("note-node", &[("node", "A4\n")]), "node");
    assert!(named.is_ok(), "a note name is still the frequency it names");
}
