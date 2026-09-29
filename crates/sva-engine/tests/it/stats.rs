// Concern: proves a render reports every lookup it made and what each came to | Non-concern: the store's cap and prunes (stores.rs) | IO: (a composition, a store) -> CacheStats

use std::collections::BTreeSet;

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Cache, CacheStats, Hash, RenderConfig, render};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// Three voices over one filtered pad, so two of them share everything under `pad`.
fn demo(name: &str) -> Graph {
    graph_of(
        name,
        &[
            ("osc", "saw(110*t) + 0.5*square(165*t)\n"),
            ("pad", "lowpass(sample(@osc), cutoff=900, q=0.8)\n"),
            ("a", "@pad*0.8\n"),
            ("b", "@pad*0.5 + sample(sin(2*pi*440*t))*0.2\n"),
            ("c", "tanh(@pad*2)*0.5\n"),
        ],
    )
}

fn stats(graph: &Graph, root: &str, cache: &Cache) -> CacheStats {
    render(
        graph,
        root,
        RenderConfig::seconds(RATE, SECONDS),
        Some(cache),
    )
    .unwrap_or_else(|e| panic!("rendering `{root}`: {e}"))
    .cache_stats
    .expect("a render handed a store reports on it")
}

fn keys(stats: &CacheStats) -> BTreeSet<(Hash, String)> {
    stats
        .lookups
        .iter()
        .map(|l| (l.key, format!("{}:{:?}", l.node, l.kind)))
        .collect()
}

/// With no store, a render still notes each read, every one after the first a reuse.
#[test]
fn a_render_handed_no_store_reports_its_own_reuse_and_no_store() {
    let graph = demo("no-store");
    let held = render(&graph, "b", RenderConfig::seconds(RATE, SECONDS), None).expect("a render");
    let stats = held
        .cache_stats
        .expect("every render reports what it asked");
    assert_eq!((stats.bytes, stats.entries), (0, 0));
    assert!(stats.computed() > 0 && stats.stored() == 0, "{stats:?}");
    let pad: Vec<_> = stats.lookups.iter().filter(|l| l.node == "pad").collect();
    assert_eq!(pad.len(), 1, "one read of the pad: {stats:?}");
}

/// A second render finds the target the first stored, so nothing under it is asked.
#[test]
fn an_identical_second_render_is_all_hits() {
    let graph = demo("identical");
    let cache = Cache::new();
    let cold = stats(&graph, "b", &cache);
    let warm = stats(&graph, "b", &cache);
    assert!(!warm.lookups.is_empty());
    assert!(
        keys(&warm).is_subset(&keys(&cold)),
        "lookups the first render asked"
    );
    assert_eq!(warm.hits(), warm.lookups.len());
    assert_eq!(warm.computed(), 0);
}
