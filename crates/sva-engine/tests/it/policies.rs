// Concern: proves each cache policy keeps what it names, and a warm render under any is the cold one | Non-concern: evicting (stores.rs) | IO: (a composition, a policy) -> what memory holds

use std::collections::BTreeSet;

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{CachePolicy, Hash, Outcome, Render, RenderConfig, Tier, render};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;

/// `x` and `y` are each read by `a` and `b`, so they are the forks; `master` is the target.
fn forked(master: &str) -> Graph {
    graph_of(
        "policies",
        &[
            ("x", "sample(sin(2*pi*110*t))*0.5\n"),
            ("y", "sample(sin(2*pi*111*t))*0.5\n"),
            ("a", "@x*0.5 + @y*0.25\n"),
            ("b", "@x*0.25 + @y*0.5\n"),
            ("master", master),
        ],
    )
}

fn rendered(graph: &Graph, cache: &Tier) -> Render {
    let config = RenderConfig::seconds(RATE, SECONDS);
    render(graph, "master", config, cache).expect("a render")
}

/// The node's own value's key, as the store holds it: every subterm it holds is looked up
/// before it, dependencies first.
fn own(render: &Render, node: &str) -> Hash {
    let stats = render
        .cache_stats
        .as_ref()
        .expect("a render handed a store");
    stats
        .lookups
        .iter()
        .rev()
        .find(|l| l.node == node)
        .unwrap_or_else(|| panic!("`{node}` was looked up: {stats:?}"))
        .key
}

fn stored(render: &Render) -> BTreeSet<Hash> {
    let stats = render
        .cache_stats
        .as_ref()
        .expect("a render handed a store");
    stats
        .lookups
        .iter()
        .filter(|l| l.store.is_none() && l.outcome == Outcome::ComputedStored)
        .map(|l| l.key)
        .collect()
}

fn root(render: &Render) -> Vec<f64> {
    render
        .output(render.root)
        .expect("the root")
        .plane(0)
        .to_vec()
}

#[test]
fn each_policy_stores_what_it_names() {
    let graph = forked("@a + @b\n");
    for policy in CachePolicy::ALL {
        let cache = Tier::default();
        cache.set_policy(policy);
        let cold = rendered(&graph, &cache);
        let stats = cold.cache_stats.as_ref().expect("stats");
        let named: BTreeSet<Hash> = match policy {
            CachePolicy::All => {
                assert_eq!(stats.stored(), stats.computed(), "{stats:?}");
                continue;
            }
            CachePolicy::Forks => [own(&cold, "x"), own(&cold, "y"), own(&cold, "master")].into(),
            CachePolicy::Target => [own(&cold, "master")].into(),
        };
        assert_eq!(stored(&cold), named, "{policy:?}");
        assert!(named.iter().all(|key| cache.holds(*key)), "{policy:?}");
    }
}

#[test]
fn a_warm_render_is_the_cold_one_byte_for_byte_under_every_policy() {
    let graph = forked("@a + @b\n");
    let uncached = rendered(&graph, &Tier::default());
    let cold = root(&uncached);
    for policy in CachePolicy::ALL {
        let cache = Tier::default();
        cache.set_policy(policy);
        assert_eq!(root(&rendered(&graph, &cache)), cold, "{policy:?}");
        let warm = rendered(&graph, &cache);
        assert_eq!(root(&warm), cold, "{policy:?} warm");
        assert_eq!(
            warm.labels[&warm.root].detail, uncached.labels[&uncached.root].detail,
            "{policy:?}: a hit carries the cold label"
        );
    }
}

/// A value the store answers covers everything it was built from: nothing under it is run.
#[test]
fn a_hit_covers_what_it_was_built_from() {
    let cache = Tier::default();
    cache.set_policy(CachePolicy::Target);
    rendered(&forked("@a + @b\n"), &cache);
    let warm = rendered(&forked("@a + @b\n"), &cache);
    let stats = warm.cache_stats.expect("stats");
    assert_eq!((stats.lookups.len(), stats.hits()), (1, 1), "{stats:?}");

    let cache = Tier::default();
    cache.set_policy(CachePolicy::Forks);
    rendered(&forked("@a + @b\n"), &cache);
    let edited = rendered(&forked("@a - @b\n"), &cache);
    let stats = edited.cache_stats.as_ref().expect("stats");
    let outcome = |key: Hash| {
        stats
            .lookups
            .iter()
            .find(|l| l.key == key)
            .map(|l| l.outcome)
    };
    for fork in ["x", "y"] {
        let hit = |l: &&sva_engine::Lookup| l.node == fork && l.outcome == Outcome::Hit;
        assert!(stats.lookups.iter().any(|l| hit(&l)), "`{fork}`: {stats:?}");
    }
    for single in ["a", "b"] {
        assert_eq!(
            outcome(own(&edited, single)),
            Some(Outcome::ComputedNotStored),
            "`{single}` is run again and not kept"
        );
    }
}
