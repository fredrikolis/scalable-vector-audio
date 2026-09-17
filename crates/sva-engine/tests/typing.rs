// Concern: proves every node across a graph gets one type, and that one inference decides it | Non-concern: the refusals a crossing writes (refusals.rs) | IO: (a composition) -> Ty per node

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Held, Var, check_structure, types};

const CHORD: &str = "sin(2*pi*261.63*t) + sin(2*pi*329.63*t) + sin(2*pi*392*t)\n";

#[test]
fn a_graph_of_closed_forms_types_end_to_end() {
    let g = graph_of(
        "laws",
        &[
            ("chord", CHORD),
            ("env", "crop(exp(0 - 4*t), 0s, 4s)\n"),
            ("voiced", "@chord*@env\n"),
            ("master", "lowpass(@voiced, 800, 0.7)*0.5\n"),
        ],
    );
    let typing = types(&g, "master").expect("a graph of laws");
    for path in ["chord", "env", "voiced", "master"] {
        let id = typing.id(path).unwrap_or_else(|| panic!("{path} is typed"));
        assert!(
            typing.ty(id).is_closed_form(),
            "{path} is a closed form, got {:?}",
            typing.ty(id)
        );
        assert_eq!(typing.ty(id).width, 1, "{path}");
    }
    let chord = typing.ty(typing.id("chord").expect("chord"));
    assert_eq!(
        (chord.held, chord.dual),
        (Held::Form(Var::T), true),
        "three sines are a closed form in t with a dual"
    );
}

#[test]
fn a_node_touching_sp_is_discrete() {
    let g = graph_of(
        "grid",
        &[("chord", CHORD), ("tail", "@chord(t - 1sp)*0.5\n")],
    );
    let typing = types(&g, "tail").expect("a grid read");
    let tail = typing.id("tail").expect("tail");
    assert_eq!(
        typing.ty(tail).held,
        Held::Sampled,
        "an sp offset puts the whole node on the grid"
    );
    let chord = typing.ty(typing.id("chord").expect("chord"));
    assert_eq!(
        (chord.held, chord.dual),
        (Held::Form(Var::T), true),
        "the closed form it reads is untouched"
    );
}

/// The judgment is structural over the spectral sum's existence, so nothing a second run
/// holds — no cached term, no already-decided node — can change a type.
#[test]
fn a_dual_is_decided_identically_by_lint_and_by_render() {
    let g = graph_of(
        "identical",
        &[
            ("chord", CHORD),
            ("drive", "tanh(@chord*4)\n"),
            ("mask", "1/(1 + pow(f/300, 8))\n"),
            ("master", "@drive*0.5\n"),
        ],
    );
    check_structure(&g, "master").expect("lint types it");
    let once = types(&g, "master").expect("a first inference");
    let twice = types(&g, "master").expect("a second inference");
    assert_eq!(once, twice, "two runs decide identically");
    let drive = once.ty(once.id("drive").expect("drive"));
    assert_eq!(
        (drive.held, drive.dual),
        (Held::Form(Var::T), false),
        "a bounded nonlinearity leaves A on both paths"
    );
    let masked = types(&g, "mask").expect("a closed form in f types from its own root");
    let mask = masked.ty(masked.id("mask").expect("mask"));
    assert_eq!((mask.held, mask.dual), (Held::Form(Var::F), false));
}

/// FORMAT 6.1: a finite sum has the type of its term, and a power of two in the index is a
/// constant per term, so a Shepard stack's frequency stays affine in `t` and the sum dual.
#[test]
fn a_shepard_sum_has_a_dual() {
    let g = graph_of(
        "shepard",
        &[
            ("octaves", "sum(k, 0, 8, sin(2*pi*55*pow(2, k)*t))\n"),
            ("written", "sum(k, 0, 8, sin(2*pi*55*exp(0.6931*k)*t))\n"),
            ("glide", "sin(2*pi*55*pow(2, t/8)*t)\n"),
            ("master", "@octaves + @written + @glide\n"),
        ],
    );
    let typing = types(&g, "master").expect("a graph of laws");
    for path in ["octaves", "written"] {
        let id = typing.id(path).expect("a typed node");
        assert!(
            typing.ty(id).has_dual(),
            "{path}: an exponential in the index is one frequency per term"
        );
    }
    let glide = typing.ty(typing.id("glide").expect("a typed node"));
    assert_eq!(
        (glide.held, glide.dual),
        (Held::Form(Var::T), false),
        "an exponential in t is not affine in t and keeps no dual"
    );
}
