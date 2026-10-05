// Concern: proves a render reports every lookup it made and what each came to | Non-concern: memory's cap and evictions (stores.rs) | IO: (a composition, a tier) -> CacheStats

use std::collections::BTreeSet;

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{CacheStats, Hash, Outcome, RenderConfig, Tier, render};

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

fn stats(graph: &Graph, root: &str, cache: &Tier) -> CacheStats {
    render(graph, root, RenderConfig::seconds(RATE, SECONDS), cache)
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

/// Over a memory that keeps nothing, a render still notes each read, every one after the first
/// a reuse.
#[test]
fn a_render_over_a_memory_keeping_nothing_reports_its_own_reuse() {
    let graph = demo("no-store");
    let held = render(
        &graph,
        "b",
        RenderConfig::seconds(RATE, SECONDS),
        &Tier::new(0),
    )
    .expect("a render");
    let stats = held
        .cache_stats
        .expect("every render reports what it asked");
    assert_eq!((stats.bytes, stats.entries), (0, 0));
    assert!(stats.computed() > 0 && stats.stored() == 0, "{stats:?}");
    let pad = stats.lookups.iter().filter(|l| l.node == "pad");
    let pad: Vec<_> = pad.filter(|l| l.outcome != Outcome::Reused).collect();
    assert_eq!(pad.len(), 1, "one read of the pad: {stats:?}");
}

/// The walk asks a node's key and the value graph asks the same key of its value: one lookup.
#[test]
fn a_render_looks_each_key_up_once() {
    let stats = stats(&demo("once"), "b", &Tier::default());
    let looked: Vec<Hash> = stats
        .lookups
        .iter()
        .filter(|l| l.outcome != Outcome::Reused)
        .map(|l| l.key)
        .collect();
    let distinct: BTreeSet<Hash> = looked.iter().copied().collect();
    assert_eq!(looked.len(), distinct.len(), "{stats:?}");
    assert_eq!(stats.hits() + stats.computed(), looked.len());
}

/// A second render finds the target the first stored, so nothing under it is asked.
#[test]
fn an_identical_second_render_is_all_hits() {
    let graph = demo("identical");
    let cache = Tier::default();
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

/// A read is looked up when its reader first writes the samples it feeds: a note placed at
/// 2 s and at 4 s is reused there, not where the render begins.
#[test]
fn a_reuse_is_noted_where_its_read_first_sounds() {
    let graph = graph_of(
        "reuse-placed",
        &[
            ("note", "crop(sin(2*pi*440*t)*exp(-t/0.2), 0s, 0.5s)\n"),
            ("song", "@note(t) + @note(t - 2s) + @note(t - 4s)\n"),
        ],
    );
    let one_block_a_pull = RenderConfig {
        threads: std::num::NonZeroUsize::MIN,
        ..RenderConfig::seconds(RATE, 6.0)
    };
    let held = render(&graph, "song", one_block_a_pull, &Tier::default()).expect("a render");
    let stats = held
        .cache_stats
        .expect("every render reports what it asked");
    let made_by = |secs: f64| {
        let at = (secs * f64::from(RATE)) as i64;
        stats
            .reached
            .iter()
            .take_while(|(reached, _)| *reached <= at)
            .last()
            .map_or(0, |(_, made)| *made)
    };
    let reuses: Vec<usize> = (0..stats.lookups.len())
        .filter(|&i| stats.lookups[i].node == "note" && stats.lookups[i].outcome == Outcome::Reused)
        .collect();
    assert_eq!(reuses.len(), 2, "{stats:?}");
    for (at, secs) in reuses.into_iter().zip([2.0, 4.0]) {
        assert!(
            made_by(secs - 0.5) <= at && at < made_by(secs + 0.5),
            "the read at {secs} s was noted as lookup {at}: {:?}",
            stats.reached
        );
    }
}

/// A node that only moves another is that node read: two reads of it are two lookups of the
/// node it moves, the second a reuse.
#[test]
fn a_read_through_a_moved_node_is_a_lookup_of_the_node_it_moves() {
    let graph = graph_of(
        "reuse-moved",
        &[
            ("a", "crop(sin(2*pi*440*t)*exp(-t/0.1), 0s, 0.5s)\n"),
            ("b", "@a(t - 1s)\n"),
            ("song", "@b(t) + @b(t - 2s)\n"),
        ],
    );
    let held = render(
        &graph,
        "song",
        RenderConfig::seconds(RATE, 4.0),
        &Tier::new(0),
    )
    .expect("a render");
    let stats = held
        .cache_stats
        .expect("every render reports what it asked");
    let of = |node: &str| {
        stats
            .lookups
            .iter()
            .filter(|l| l.node == node)
            .map(|l| l.outcome)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        of("a"),
        [Outcome::ComputedNotStored, Outcome::Reused],
        "{stats:?}"
    );
}
