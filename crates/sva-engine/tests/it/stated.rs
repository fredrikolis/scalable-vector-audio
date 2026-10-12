// Concern: proves a stored node states its support and its open end, so a render it answers analyses neither | Non-concern: what memory keeps (persist.rs) | IO: (composition, fake disk) -> counts, bits

use std::num::NonZeroUsize;

use crate::disk::{Memory, now, opened};
use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{CacheStats, Outcome, Range, Render, RenderConfig, Tier, render_over};

const RATE: u32 = 8_000;

/// Two endless decaying voices: read open, it ends where it is treated as silent.
fn decaying(name: &str, gain: &str) -> Graph {
    graph_of(
        name,
        &[
            (
                "ping",
                "lowpass(sample(crop(sin(2*pi*440*t)*exp(-t/0.01), 0s, inf)), cutoff=2000, q=0.7)\n",
            ),
            (
                "pong",
                "lowpass(sample(crop(sin(2*pi*660*t)*exp(-t/0.02), 0s, inf)), cutoff=2000, q=0.7)\n",
            ),
            ("master", &format!("@ping*{gain} + @pong(t - 0.01s)*0.25\n")),
        ],
    )
}

fn open() -> RenderConfig {
    RenderConfig {
        threads: NonZeroUsize::new(1).expect("one"),
        ..RenderConfig::at(RATE)
    }
}

fn closed(end: i64) -> RenderConfig {
    let range = Range {
        start: Some(0),
        end: Some(end),
    };
    RenderConfig { range, ..open() }
}

fn rendered<B: sva_engine::Backend>(graph: &Graph, config: RenderConfig, tier: &Tier<B>) -> Render {
    now(render_over(graph, "master", config, tier)).expect("a render")
}

fn stored(memory: &Memory, graph: &Graph, config: RenderConfig) -> Render {
    let store = opened(memory, u64::MAX);
    let held = rendered(graph, config, &store);
    now(store.persist()).expect("persisted");
    held
}

fn stats(render: &Render) -> &CacheStats {
    render.cache_stats.as_ref().expect("a render over memory")
}

fn bits(render: &Render) -> Vec<Vec<u64>> {
    let root = render.output(render.root).expect("the root's samples");
    let planes = root.planes.iter();
    planes
        .map(|plane| plane.iter().map(|v| v.to_bits()).collect())
        .collect()
}

fn same(warm: &Render, cold: &Render) {
    assert_eq!(warm.range, cold.range);
    assert_eq!(bits(warm), bits(cold));
    assert_eq!(warm.labels[&warm.root], cold.labels[&cold.root]);
}

/// The next process looks the root up once and analyses nothing, for the same range, bits and
/// label; every node is still typed first, so a typing refusal stands.
#[test]
fn a_root_stored_read_open_tells_the_next_process_where_it_ends() {
    let memory = Memory::default();
    let graph = decaying("stated-open", "0.5");
    let cold = stored(&memory, &graph, open());
    let label = &cold.labels[&cold.root];
    let cutting = label.cutting_below_silence_threshold.as_ref();
    let cut = cutting.map(|c| c.treated_as_silent_from_sample.clone());
    let ended = cold.range.expect("a range").end;
    assert_eq!(cut, Some(vec![("master".to_string(), ended)]));
    assert!(stats(&cold).analysed > 0, "{:?}", stats(&cold));

    let warm = rendered(&graph, open(), &opened(&memory, u64::MAX));
    let found = stats(&warm);
    assert_eq!(found.analysed, 0, "the stored root told its end");
    assert_eq!(found.computed(), 0, "{found:?}");
    let looked: Vec<&str> = found.lookups.iter().map(|l| l.node.as_str()).collect();
    assert_eq!(
        looked,
        ["master"],
        "the root answers, nothing under it is asked"
    );
    let mut typed = found.typed.clone();
    typed.sort_unstable();
    assert_eq!(typed, ["master", "ping", "pong"]);
    same(&warm, &cold);
}

/// A reader at another silence threshold analyses its own end, as with no store.
#[test]
fn a_reader_at_another_silence_threshold_finds_its_own_end() {
    let memory = Memory::default();
    let graph = decaying("stated-threshold", "0.5");
    let cold = stored(&memory, &graph, open());
    let mut louder = open();
    louder.profile.silence_threshold_dbfs = -60.0;

    let warm = rendered(&graph, louder.clone(), &opened(&memory, u64::MAX));
    let alone = rendered(&graph, louder, &Tier::default());
    assert!(stats(&warm).analysed > 0, "{:?}", stats(&warm));
    assert_ne!(alone.range, cold.range, "another threshold, another end");
    same(&warm, &alone);
}

/// Read over its interval again it analyses nothing; read open, its end is analysed as with
/// no store, and the interval stored answers it.
#[test]
fn a_root_stored_over_a_closed_interval_states_its_support_and_no_end() {
    let memory = Memory::default();
    let graph = decaying("stated-closed", "0.5");
    let alone = rendered(&graph, open(), &Tier::default());
    let end = alone.range.expect("a range").end;
    let cold = stored(&memory, &graph, closed(end));
    assert!(stats(&cold).analysed > 0, "{:?}", stats(&cold));

    let again = rendered(&graph, closed(end), &opened(&memory, u64::MAX));
    assert_eq!(stats(&again).analysed, 0, "{:?}", stats(&again));
    assert_eq!(bits(&again), bits(&cold));

    let read_open = rendered(&graph, open(), &opened(&memory, u64::MAX));
    assert!(stats(&read_open).analysed > 0, "{:?}", stats(&read_open));
    assert_eq!(stats(&read_open).computed(), 0, "{:?}", stats(&read_open));
    same(&read_open, &alone);
}

/// Under an edited root, each stored node states its own support: fewer are analysed.
#[test]
fn an_edited_root_analyses_no_support_a_stored_node_under_it_states() {
    let memory = Memory::default();
    stored(
        &memory,
        &decaying("stated-before", "0.5"),
        closed(i64::from(RATE)),
    );
    let edited = decaying("stated-after", "0.4");

    let warm = rendered(&edited, closed(i64::from(RATE)), &opened(&memory, u64::MAX));
    let alone = rendered(&edited, closed(i64::from(RATE)), &Tier::default());
    for node in ["ping", "pong"] {
        let hit = |l: &&sva_engine::Lookup| l.node == node && l.outcome == Outcome::Hit;
        assert!(stats(&warm).lookups.iter().any(|l| hit(&l)), "`{node}`");
    }
    assert!(stats(&warm).analysed > 0, "the edited root is analysed");
    assert!(
        stats(&warm).analysed < stats(&alone).analysed,
        "{} then {}",
        stats(&warm).analysed,
        stats(&alone).analysed
    );
    same(&warm, &alone);
}
