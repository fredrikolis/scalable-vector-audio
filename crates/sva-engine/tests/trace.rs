// Concern: proves a trace names what a node is built from, who reads it, and the type it holds | Non-concern: measuring it, which needs audio | IO: (a composition, target) -> Traced

mod fixtures;

use fixtures::graph_of;
use sva_engine::{EngineError, trace};

const CHORD: &str = "sin(2*pi*256*t) + sin(2*pi*512*t)\n";

#[test]
fn a_trace_names_what_a_node_reads_and_who_reads_it() {
    let g = graph_of(
        "structure",
        &[
            ("chord", CHORD),
            ("voiced", "@chord*0.5\n"),
            ("master", "@voiced + @chord\n"),
        ],
    );
    let traced = trace(&g, &["master".to_string()], "voiced").expect("a trace");
    assert_eq!(traced.node, "voiced");
    assert_eq!(traced.down, vec!["chord".to_string()]);
    assert_eq!(
        traced
            .up
            .iter()
            .map(|u| u.node.as_str())
            .collect::<Vec<_>>(),
        vec!["master"]
    );
    assert_eq!(traced.entry, vec!["master".to_string()]);
    assert!(traced.cycle.is_none());
}

/// A trace is what a composer reads before rendering anything, so it says which
/// representation each node holds.
#[test]
fn a_trace_prints_the_type_each_node_holds() {
    let g = graph_of(
        "typed",
        &[
            ("chord", CHORD),
            ("grid", "chaigne_askenfelt(261.63)\n"),
            ("master", "sample(@chord) + @grid\n"),
        ],
    );
    let traced = trace(&g, &["master".to_string()], "master").expect("a trace");
    assert_eq!(traced.ty, "samples");
    let closed = trace(&g, &["master".to_string()], "chord").expect("a trace");
    assert_eq!(closed.ty, "a closed form in `t` with a dual");
    assert!(
        closed.up.iter().all(|u| !u.ty.is_empty()),
        "every reader prints its own type"
    );
}

#[test]
fn a_file_several_tuples_share_refuses_naming_them() {
    let g = graph_of(
        "ambiguous",
        &[
            ("src", CHORD),
            ("gain", "x*k\n"),
            ("master", "@gain(t, x=@src, k=1) + @gain(t, x=@src, k=2)\n"),
        ],
    );
    let refused = trace(&g, &["master".to_string()], "gain");
    assert!(
        matches!(refused, Err(EngineError::AmbiguousNode(file, names)) if file == "gain" && names.len() == 2)
    );
}
