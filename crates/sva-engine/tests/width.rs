// Concern: proves a width-1 operand broadcasts into every lane of the operator it meets | Non-concern: how a width is inferred (typing.rs) | IO: (a composition) -> the planes each channel holds

mod fixtures;

use fixtures::graph_of;
use sva_engine::{RenderConfig, render};

fn planes(name: &str, master: &str) -> (Vec<f64>, Vec<f64>) {
    let g = graph_of(
        name,
        &[
            ("pair", "join(sin(2*pi*400*t), sin(2*pi*410*t))\n"),
            ("mono", "0.5*sin(2*pi*220*t)\n"),
            ("master", master),
        ],
    );
    let held = render(&g, "master", RenderConfig::seconds(8_000, 1.0), None)
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let id = held.id("master").expect("the root");
    let buffer = held.buffer(id).expect("a rendered master");
    assert_eq!(buffer.width, 2, "{name} is a pair");
    (buffer.plane(0).to_vec(), buffer.plane(1).to_vec())
}

/// A mono term summed with a pair is added to both lanes, not to the first alone.
#[test]
fn a_mono_term_plus_a_stereo_term_sounds_in_both_channels() {
    let (left, right) = planes("mono-plus-pair", "@pair(t) + @mono(t)\n");
    let (bare_left, bare_right) = planes("pair-alone", "@pair(t)\n");
    let (joined_left, joined_right) = planes("mono-joined", "@pair(t) + join(1, 1)*@mono(t)\n");

    let (first_left, first_right) = planes("mono-first", "@mono(t) + @pair(t)\n");
    assert_ne!(right, bare_right, "the mono term reaches the right channel");
    assert_eq!(
        (first_left, first_right),
        (left.clone(), right.clone()),
        "which side of the sum the mono term is written on settles nothing"
    );
    assert_eq!(
        left, joined_left,
        "and is what `join(1, 1)*` spelled by hand"
    );
    assert_eq!(right, joined_right);
    let energy = |p: &[f64]| p.iter().map(|x| x * x).sum::<f64>();
    assert!(
        energy(&left) > energy(&bare_left) && energy(&right) > energy(&bare_right),
        "both channels gained the mono term"
    );
}

/// The same rule under a product: one lane times a pair is that lane against both.
#[test]
fn a_mono_term_times_a_stereo_term_broadcasts() {
    let (left, right) = planes("mono-times-pair", "@pair(t)*@mono(t)\n");
    let (joined_left, joined_right) = planes("times-joined", "@pair(t)*join(1, 1)*@mono(t)\n");
    assert_eq!(left, joined_left);
    assert_eq!(right, joined_right);
    assert!(
        right.iter().any(|x| *x != 0.0),
        "the right channel is not silent"
    );
}
