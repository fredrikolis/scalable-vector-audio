// Concern: proves a render reports every lookup it made and what each came to | Non-concern: a store's medium or budget (stores.rs, pack.rs) | IO: (a composition, a store) -> CacheStats

mod fixtures;

use std::collections::BTreeSet;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Cache, CacheStats, Hash, MemoryCache, Outcome, PayloadKind, RenderConfig, Tier, render,
};

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

fn stats(graph: &Graph, root: &str, cache: &dyn Cache) -> CacheStats {
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

#[test]
fn a_render_handed_no_store_reports_no_stats() {
    let graph = demo("no-store");
    let held = render(&graph, "a", RenderConfig::seconds(RATE, SECONDS), None).expect("a render");
    assert_eq!(held.cache_stats, None);
}

#[test]
fn a_second_voice_hits_exactly_what_it_shares_with_the_first_and_stores_the_rest() {
    let graph = demo("shared");
    let cache = MemoryCache::new();
    let first = stats(&graph, "a", &cache);
    assert_eq!(first.hits(), 0, "a cold store answers nothing");
    assert_eq!(first.stored(), first.lookups.len(), "and keeps everything");

    let second = stats(&graph, "b", &cache);
    let before: BTreeSet<Hash> = first.lookups.iter().map(|l| l.key).collect();
    let (shared, own): (Vec<_>, Vec<_>) =
        second.lookups.iter().partition(|l| before.contains(&l.key));
    assert!(!shared.is_empty(), "b reads the pad a rendered");
    assert!(
        shared
            .iter()
            .any(|l| l.node == "osc" && l.kind == PayloadKind::Symbolic),
        "the oscillator's spectral sum is shared: {shared:?}"
    );
    for lookup in &shared {
        assert_eq!(lookup.outcome, Outcome::Hit(Tier::Memory), "{lookup:?}");
    }
    assert!(!own.is_empty(), "b has a voice of its own");
    for lookup in &own {
        assert_eq!(lookup.outcome, Outcome::ComputedStored, "{lookup:?}");
    }
    assert_eq!(second.hits(), shared.len());
    assert_eq!(second.hits_in(Tier::Persistent), 0);
    assert_eq!(second.computed(), own.len());
    assert_eq!(second.stored(), own.len());
}

#[test]
fn an_identical_second_render_is_all_hits() {
    let graph = demo("identical");
    let cache = MemoryCache::new();
    let cold = stats(&graph, "b", &cache);
    let warm = stats(&graph, "b", &cache);
    assert!(!warm.lookups.is_empty());
    assert_eq!(keys(&cold), keys(&warm), "the same lookups, asked again");
    assert_eq!(warm.hits(), warm.lookups.len());
    assert_eq!(warm.computed(), 0);
    assert_eq!(warm.nodes(), cold.nodes());
}
