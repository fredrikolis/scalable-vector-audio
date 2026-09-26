// Concern: proves each cache policy stores what it names, and a warm render under any is the cold one | Non-concern: evicting (stores.rs) | IO: (a composition, a policy) -> what the store holds

mod fixtures;

use std::collections::BTreeSet;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Cache, CachePolicy, Hash, Outcome, Render, RenderConfig, buffer_key, identity, render,
};
use sva_samples::AliasScore;

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

fn rendered(graph: &Graph, cache: Option<&Cache>, policy: Option<CachePolicy>) -> Render {
    let mut config = RenderConfig::seconds(RATE, SECONDS);
    config.cache_policy = policy;
    render(graph, "master", config, cache).expect("a render")
}

/// The node's own buffer key, as the store holds it.
fn own(render: &Render, node: &str) -> Hash {
    let id = render.id(node).expect("the node types");
    buffer_key(
        identity(&render.tys, id).expect("an identity"),
        RATE,
        0.0,
        (SECONDS * f64::from(RATE)).round() as usize,
        render.tys.ty(id).width as usize,
        AliasScore::NotAsked,
    )
}

fn stored(render: &Render) -> BTreeSet<Hash> {
    let stats = render
        .cache_stats
        .as_ref()
        .expect("a render handed a store");
    stats
        .lookups
        .iter()
        .filter(|l| l.outcome == Outcome::ComputedStored)
        .map(|l| l.key)
        .collect()
}

fn root(render: &Render) -> Vec<f64> {
    render
        .buffer(render.root)
        .expect("the root")
        .plane(0)
        .to_vec()
}

#[test]
fn each_policy_stores_what_it_names() {
    let graph = forked("@a + @b\n");
    for policy in CachePolicy::ALL {
        let cache = Cache::new();
        cache.set_policy(policy);
        let cold = rendered(&graph, Some(&cache), None);
        let stats = cold.cache_stats.as_ref().expect("stats");
        let named: BTreeSet<Hash> = match policy {
            CachePolicy::All => {
                assert_eq!(stats.stored(), stats.computed(), "{stats:?}");
                continue;
            }
            CachePolicy::Forks => [own(&cold, "x"), own(&cold, "y"), own(&cold, "master")].into(),
            CachePolicy::Target => [own(&cold, "master")].into(),
            CachePolicy::None => BTreeSet::new(),
        };
        assert_eq!(stored(&cold), named, "{policy:?}");
        assert_eq!(cache.entries(), named.len(), "{policy:?}");
    }
}

#[test]
fn a_render_names_its_own_policy_over_the_stores() {
    let graph = forked("@a + @b\n");
    let cache = Cache::new();
    let held = rendered(&graph, Some(&cache), Some(CachePolicy::None));
    assert!(stored(&held).is_empty());
    assert_eq!(cache.entries(), 0);
    let held = rendered(&graph, Some(&cache), Some(CachePolicy::Target));
    assert_eq!(stored(&held), [own(&held, "master")].into());
}

#[test]
fn a_warm_render_is_the_cold_one_byte_for_byte_under_every_policy() {
    let graph = forked("@a + @b\n");
    let uncached = rendered(&graph, None, None);
    let cold = root(&uncached);
    for policy in CachePolicy::ALL {
        let cache = Cache::new();
        cache.set_policy(policy);
        assert_eq!(
            root(&rendered(&graph, Some(&cache), None)),
            cold,
            "{policy:?}"
        );
        let warm = rendered(&graph, Some(&cache), None);
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
    let cache = Cache::new();
    cache.set_policy(CachePolicy::Target);
    rendered(&forked("@a + @b\n"), Some(&cache), None);
    let warm = rendered(&forked("@a + @b\n"), Some(&cache), None);
    let stats = warm.cache_stats.expect("stats");
    assert_eq!((stats.lookups.len(), stats.hits()), (1, 1), "{stats:?}");

    let cache = Cache::new();
    cache.set_policy(CachePolicy::Forks);
    let first = rendered(&forked("@a + @b\n"), Some(&cache), None);
    let edited = rendered(&forked("@a - @b\n"), Some(&cache), None);
    let stats = edited.cache_stats.as_ref().expect("stats");
    let outcome = |key: Hash| {
        stats
            .lookups
            .iter()
            .find(|l| l.key == key)
            .map(|l| l.outcome)
    };
    for fork in ["x", "y"] {
        assert_eq!(outcome(own(&first, fork)), Some(Outcome::Hit), "`{fork}`");
    }
    for single in ["a", "b"] {
        assert_eq!(
            outcome(own(&edited, single)),
            Some(Outcome::ComputedNotStored),
            "`{single}` is run again and not kept"
        );
    }
}
