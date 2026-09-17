// Concern: proves a reading may name the file a composer wrote, not only the instance it expanded into | Non-concern: which instances exist (instantiate.rs) | IO: (a file name) -> a reading

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Ask, EngineError, Output, RenderConfig, Representation, answer, render};

/// One instance under another name still answers to the name on disk.
#[test]
fn a_node_with_a_default_line_renders_by_name() {
    let g = graph_of(
        "defaulted",
        &[
            ("voice", "f0 = A4\nsin(2*pi*f0*t)\n"),
            ("master", "@voice\n"),
        ],
    );
    let held = render(
        &g,
        "voice",
        RenderConfig::seconds(44_100, 0.25).asking(vec![Ask {
            node: "voice".to_string(),
            representation: Representation::Lines,
        }]),
        None,
    )
    .expect("a node that declares defaults renders");
    let id = held.id("voice").expect("the file names its one instance");
    let Output::Lines(lines) = answer(&held, id, Representation::Lines)
        .expect("lines")
        .value
    else {
        panic!("expected a line list");
    };
    assert!(
        lines.iter().any(|l| (l.hz - 440.0).abs() < 1e-9),
        "the default sounds: {lines:?}"
    );
}

/// A file two tuples share is named by neither, and the refusal says which two.
#[test]
fn a_file_two_invocations_share_is_named_by_neither() {
    let g = graph_of(
        "shared",
        &[
            ("voice", "f0 = A4\nsin(2*pi*f0*t)\n"),
            ("master", "@voice + @voice(t, f0=220)\n"),
        ],
    );
    let held = render(&g, "master", RenderConfig::seconds(44_100, 0.05), None).expect("two voices");
    let refused = held.node("voice").expect_err("the file names neither");
    assert!(
        matches!(&refused, EngineError::AmbiguousNode(file, names) if file == "voice" && names.len() == 2),
        "{refused:?}"
    );
    assert!(held.node("master").is_ok(), "the root still answers");
}
