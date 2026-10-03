// Concern: proves memory never passes its cap and evicts its least recent entries first | Non-concern: what a key stands for (cache.rs) | IO: (a composition, a tier) -> what it holds

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{CacheStats, Hash, RenderConfig, Tier, render};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// Two sampled voices `a` and `b` each read.
fn forked(f: u32) -> Graph {
    graph_of(
        "forked",
        &[
            ("x", &format!("sample(sin(2*pi*{f}*t))*0.5\n")),
            ("y", &format!("sample(sin(2*pi*{}*t))*0.5\n", f + 1)),
            ("a", "@x*0.5 + @y*0.25\n"),
            ("b", "@x*0.25 + @y*0.5\n"),
            ("master", "@a + @b\n"),
        ],
    )
}

fn stats(graph: &Graph, cache: &Tier) -> CacheStats {
    render(graph, "master", RenderConfig::seconds(RATE, SECONDS), cache)
        .expect("a render")
        .cache_stats
        .expect("a render handed a store reports on it")
}

fn held(cache: &Tier, keys: &[Hash]) -> bool {
    keys.iter().all(|k| cache.holds(*k))
}

fn gone(cache: &Tier, keys: &[Hash]) -> bool {
    keys.iter().all(|k| !cache.holds(*k))
}

fn within(cache: &Tier) {
    assert!(
        cache.bytes() <= cache.max_bytes(),
        "{} over {}",
        cache.bytes(),
        cache.max_bytes()
    );
}

#[test]
fn the_cap_holds_after_every_render_and_a_lowered_cap() {
    let cache = Tier::new(20_000);
    let mut evicted = 0;
    for f in (1..=8).map(|k| 100 * k) {
        let after = stats(&forked(f), &cache);
        within(&cache);
        assert!(after.bytes <= after.max_bytes, "{after:?}");
        assert_eq!(after.entries, cache.entries());
        evicted += after.evictions;
    }
    assert!(evicted > 0, "eight renders overflow the cap");
    assert_eq!(evicted, cache.evictions(), "every eviction is reported");
    cache.set_max_bytes(4_000);
    within(&cache);
}

/// The node keys a render's walk found in memory.
fn found(stats: &CacheStats) -> Vec<Hash> {
    let hits = stats.lookups.iter().filter(|l| l.store == Some(true));
    hits.map(|l| l.key).collect()
}

/// Rendered twice, a voice's nodes earn a hit; values rendered once and never asked again, each
/// render of them past probation's share and together past the cap, evict only each other.
#[test]
fn values_never_asked_again_never_evict_one_that_was() {
    let cache = Tier::new(40_000);
    stats(&forked(110), &cache);
    let hit = found(&stats(&forked(110), &cache));
    assert!(!hit.is_empty() && held(&cache, &hit));
    let one = stats(&forked(220), &Tier::new(u64::MAX)).bytes;
    assert!(
        one > 40_000 / 4,
        "one render passes probation's share: {one}"
    );
    let before = cache.counters();
    for f in (2..=12).map(|k| 110 * k) {
        stats(&forked(f), &cache);
        assert!(held(&cache, &hit), "{:?}", cache.counters());
    }
    let after = cache.counters().since(before);
    assert!(after.probation_evictions > 0, "{after:?}");
    assert_eq!(after.protected_evictions, 0, "{after:?}");
}

/// `n1` → `n2` → `n3` → a master.
fn chain(n3: &str, master: &str) -> Graph {
    graph_of(
        "chain",
        &[
            ("n1", "sample(sin(2*pi*110*t))*0.5\n"),
            ("n2", "lowpass(@n1, cutoff=900, q=0.7)\n"),
            ("n3", n3),
            ("master", master),
        ],
    )
}

fn node_key(stats: &CacheStats, node: &str) -> Hash {
    let looked = stats.lookups.iter().filter(|l| l.node == node);
    let mut nodes = looked.filter(|l| l.store.is_some());
    nodes.next().map(|l| l.key).expect("looked up as a node")
}

/// A master edited over memory hits `n3`, so under pressure `n1` and `n2` go first; `n3`
/// edited, `n2` earns its hit and stays where `n1` goes. Nothing goes without pressure.
#[test]
fn under_pressure_the_nodes_never_hit_go_first() {
    let n3 = "lowpass(@n2, cutoff=600, q=0.7)\n";
    let cache = Tier::default();
    let first = stats(&chain(n3, "@n3*0.5\n"), &cache);
    let edited = stats(&chain(n3, "@n3*0.25\n"), &cache);
    let [n1, n2, n3_key] = ["n1", "n2", "n3"].map(|n| node_key(&first, n));
    assert_eq!(found(&edited), [n3_key]);
    assert_eq!(cache.evictions(), 0, "nothing goes without pressure");
    cache.set_max_bytes(cache.bytes() / 2);
    assert!(held(&cache, &[n3_key]), "{:?}", cache.counters());
    assert!(gone(&cache, &[n1, n2]), "{:?}", cache.counters());

    let cache = Tier::default();
    stats(&chain(n3, "@n3*0.5\n"), &cache);
    let again = stats(
        &chain("lowpass(@n2, cutoff=500, q=0.7)\n", "@n3*0.5\n"),
        &cache,
    );
    assert_eq!(found(&again), [n2]);
    cache.set_max_bytes(cache.bytes() / 2);
    assert!(held(&cache, &[n2]), "{:?}", cache.counters());
    assert!(gone(&cache, &[n1]), "{:?}", cache.counters());
}
