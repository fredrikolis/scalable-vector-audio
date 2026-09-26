// Concern: proves the store never passes its cap and evicts what each prune policy names | Non-concern: what a key stands for (cache.rs) | IO: (a composition, a store) -> what it holds

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Cache, CacheStats, Hash, PrunePolicy, RenderConfig, render};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// Two sampled voices `a` and `b` each read, so each voice is a fork and nothing else is.
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

fn stats(graph: &Graph, cache: &Cache) -> CacheStats {
    render(
        graph,
        "master",
        RenderConfig::seconds(RATE, SECONDS),
        Some(cache),
    )
    .expect("a render")
    .cache_stats
    .expect("a render handed a store reports on it")
}

fn keys_of(stats: &CacheStats, node: &str) -> Vec<Hash> {
    let keys: Vec<Hash> = stats
        .lookups
        .iter()
        .filter(|l| l.node == node)
        .map(|l| l.key)
        .collect();
    assert!(!keys.is_empty(), "`{node}` was looked up: {stats:?}");
    keys
}

/// The node's own value: every subterm it holds is looked up before it, dependencies first.
fn own_key(stats: &CacheStats, node: &str) -> Hash {
    *keys_of(stats, node).last().expect("looked up")
}

fn held(cache: &Cache, keys: &[Hash]) -> bool {
    keys.iter().all(|k| cache.holds(*k))
}

fn gone(cache: &Cache, keys: &[Hash]) -> bool {
    keys.iter().all(|k| !cache.holds(*k))
}

fn within(cache: &Cache) {
    assert!(
        cache.bytes() <= cache.max_bytes(),
        "{} over {}",
        cache.bytes(),
        cache.max_bytes()
    );
}

#[test]
fn the_cap_holds_after_every_render_and_every_prune() {
    for policy in PrunePolicy::ALL {
        let cache = Cache::holding(20_000);
        cache.set_prune_policy(policy);
        let mut evicted = 0;
        for f in (1..=8).map(|k| 100 * k) {
            let after = stats(&forked(f), &cache);
            within(&cache);
            assert!(after.bytes <= after.max_bytes, "{policy:?}: {after:?}");
            assert_eq!(after.entries, cache.entries());
            evicted += after.evictions;
        }
        assert!(evicted > 0, "{policy:?}: eight renders overflow the cap");
        assert_eq!(evicted, cache.evictions(), "every eviction is reported");
        for prune in PrunePolicy::ALL {
            cache.prune(prune);
            within(&cache);
        }
        cache.set_max_bytes(4_000);
        within(&cache);
    }
}

#[test]
fn a_prune_by_oldest_keeps_only_what_the_newest_render_touched() {
    let cache = Cache::new();
    let first = stats(&forked(110), &cache);
    let second = stats(&forked(220), &cache);
    cache.prune(PrunePolicy::Oldest);
    for node in ["x", "y", "a", "b", "master"] {
        assert!(
            gone(&cache, &keys_of(&first, node)),
            "`{node}` of the first"
        );
        assert!(
            held(&cache, &keys_of(&second, node)),
            "`{node}` of the second"
        );
    }
}

#[test]
fn a_prune_by_forks_keeps_only_the_values_two_nodes_read() {
    let cache = Cache::new();
    let rendered = stats(&forked(110), &cache);
    cache.prune(PrunePolicy::Forks);
    for node in ["a", "b", "master"] {
        assert!(gone(&cache, &keys_of(&rendered, node)), "`{node}` went");
    }
    assert!(held(
        &cache,
        &[own_key(&rendered, "x"), own_key(&rendered, "y")]
    ));
}

/// Forks alone over the cap: the policy has nothing left to evict, so the oldest render's
/// values go together, both forks, though one alone would have been enough.
#[test]
fn where_a_policy_frees_too_little_whole_trees_go_oldest_first() {
    let cache = Cache::new();
    cache.set_prune_policy(PrunePolicy::Forks);
    let trees: Vec<CacheStats> = [110, 220, 330]
        .into_iter()
        .map(|f| stats(&forked(f), &cache))
        .collect();
    cache.prune(PrunePolicy::Forks);
    let forks = |tree: &CacheStats| [own_key(tree, "x"), own_key(tree, "y")];
    assert!(trees.iter().all(|tree| held(&cache, &forks(tree))));

    cache.set_max_bytes(cache.bytes() - 1);
    within(&cache);
    assert!(
        gone(&cache, &forks(&trees[0])),
        "the oldest tree went whole"
    );
    assert!(held(&cache, &forks(&trees[1])));
    assert!(held(&cache, &forks(&trees[2])));
}
