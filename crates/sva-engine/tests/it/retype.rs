// Concern: proves a session's render types only what changed since its last and its readers | Non-concern: memory's answers, a stream's changes | IO: (compositions, a session) -> typed nodes, bits

use std::collections::BTreeSet;

use crate::fixtures::{Now, graph_of};
use sva_ast::Graph;
use sva_engine::{Render, RenderConfig, Session, Tier, identity, render_in, render_over};

const RATE: u32 = 8_000;

fn config() -> RenderConfig {
    RenderConfig::seconds(RATE, 0.05)
}

/// `x` and `y` under a sum `p`, which a master samples; `x` at `hz`.
fn mixed(name: &str, hz: u32) -> Graph {
    let x = format!("sample(crop(sin(2*pi*{hz}*t), 0s, 0.05s))\n");
    graph_of(
        name,
        &[
            ("x", &x),
            ("y", "sample(crop(sin(2*pi*330*t), 0s, 0.05s))\n"),
            ("p", "@x*0.5 + @y\n"),
            ("master", "sample(@p*0.5)\n"),
        ],
    )
}

fn rendered(session: &mut Session, graph: &Graph, tier: &Tier) -> Render {
    let held = render_in(session, graph, "master", config(), tier).now();
    held.unwrap_or_else(|e| panic!("{e}"))
}

fn typed(render: &Render) -> BTreeSet<String> {
    let stats = render.cache_stats.as_ref().expect("a render over memory");
    stats.typed.iter().cloned().collect()
}

fn bits(render: &Render) -> Vec<Vec<u64>> {
    let root = render.output(render.root).expect("the root's samples");
    let planes = root.planes.iter();
    planes
        .map(|p| p.iter().map(|v| v.to_bits()).collect())
        .collect()
}

/// What a render of `graph` alone writes and names its root, to hold a session's render to.
fn alone(graph: &Graph) -> Render {
    let held = render_over(graph, "master", config(), &Tier::default()).now();
    held.unwrap_or_else(|e| panic!("{e}"))
}

fn same_as_alone(render: &Render, graph: &Graph) {
    let fresh = alone(graph);
    assert_eq!(bits(render), bits(&fresh));
    for path in ["x", "y", "p", "master"] {
        let (mine, theirs) = (
            render.id(path).expect("held"),
            fresh.id(path).expect("held"),
        );
        assert_eq!(
            identity(&render.tys, mine).expect("named"),
            identity(&fresh.tys, theirs).expect("named"),
            "`{path}` is the value a render of its own names"
        );
    }
}

/// A second render of an unchanged composition types nothing, a fresh tier or not.
#[test]
fn a_repeated_render_in_a_session_types_nothing() {
    let mut session = Session::default();
    let graph = mixed("retype-same", 220);
    let first = rendered(&mut session, &graph, &Tier::default());
    let all: BTreeSet<String> = ["x", "y", "p", "master"].map(String::from).into();
    assert_eq!(typed(&first), all);
    for tier in [Tier::default(), Tier::default()] {
        let again = rendered(&mut session, &mixed("retype-same-again", 220), &tier);
        assert_eq!(typed(&again), BTreeSet::new());
        same_as_alone(&again, &graph);
    }
}

/// An edit of one node types that node and each node reading it, and no other.
#[test]
fn a_render_after_an_edit_types_only_the_edited_node_and_its_readers() {
    let mut session = Session::default();
    let tier = Tier::default();
    rendered(&mut session, &mixed("retype-before", 220), &tier);
    let edited = mixed("retype-after", 440);
    let after = rendered(&mut session, &edited, &tier);
    let share: BTreeSet<String> = ["x", "p", "master"].map(String::from).into();
    assert_eq!(typed(&after), share);
    same_as_alone(&after, &edited);
}

/// A render refused while typing leaves the session as it was: the next types only its change.
#[test]
fn a_refused_render_leaves_the_session_as_it_was() {
    let mut session = Session::default();
    let tier = Tier::default();
    rendered(&mut session, &mixed("retype-held", 220), &tier);
    let broken = graph_of(
        "retype-broken",
        &[
            ("x", "sample(crop(sin(2*pi*440*t), 0s, 0.05s))\n"),
            ("y", "sample(crop(sin(2*pi*330*t), 0s, 0.05s))\n"),
            ("p", "@x*0.5 + @y\n"),
            ("master", "sample(@p*0.5) + lowpass(1)\n"),
        ],
    );
    let refused = render_in(&mut session, &broken, "master", config(), &tier).now();
    assert!(refused.is_err(), "a lowpass of no cutoff refuses");
    let edited = mixed("retype-mended", 440);
    let after = rendered(&mut session, &edited, &tier);
    let share: BTreeSet<String> = ["x", "p", "master"].map(String::from).into();
    assert_eq!(typed(&after), share);
    same_as_alone(&after, &edited);
}

/// A note through a tone knob at `cutoff`.
fn knob(cutoff: u32) -> Graph {
    graph_of(
        "retype-knob",
        &[
            ("note", "sample(sin(2*pi*220*t))*0.5\n"),
            ("tone", "lowpass(x, cutoff=cutoff, q=0.7)\n"),
            ("master", &format!("@tone(t, x=@note, cutoff={cutoff})\n")),
        ],
    )
}

fn turned(session: &mut Session, cutoff: u32, tier: &Tier) -> (Vec<String>, Render) {
    let mut config = config();
    config.volatile = vec!["cutoff".to_string()];
    let held = render_in(session, &knob(cutoff), "master", config.clone(), tier).now();
    let held = held.unwrap_or_else(|e| panic!("{e}"));
    let alone = render_over(&knob(cutoff), "master", config, &Tier::default()).now();
    assert_eq!(bits(&held), bits(&alone.unwrap_or_else(|e| panic!("{e}"))));
    let stats = held.cache_stats.as_ref().expect("a render over memory");
    (stats.typed.clone(), held)
}

/// A volatile knob's move types the nodes it reaches once, and the composition with the knob at
/// its stand-in not at all.
#[test]
fn a_knob_move_types_only_what_the_knob_reaches() {
    let mut session = Session::default();
    let tier = Tier::default();
    turned(&mut session, 400, &tier);
    let (moved, _) = turned(&mut session, 800, &tier);
    let reached = |path: &String| path == "master" || path.starts_with("tone(");
    assert!(moved.iter().all(reached), "{moved:?}");
    assert_eq!(moved.len(), 2, "{moved:?}");
    let (again, _) = turned(&mut session, 800, &tier);
    assert_eq!(again, Vec::<String>::new());
}
