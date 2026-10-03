// Concern: proves a warm run names every value as the cold run that stored it did | Non-concern: which framings share an entry (sva-cli's store.rs) | IO: (compositions, one disk) -> identities, bits

use crate::disk::{Memory, now, opened};
use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{CacheStats, Outcome, Render, RenderConfig, identity, render_over};

const RATE: u32 = 8_000;
const SECONDS: f64 = 0.05;
const HEADER: &str = "; Models: x | Neglects: y | IO: (t) -> amplitude | Tags: t\n";

fn rendered(graph: &Graph, target: &str, memory: &Memory) -> Render {
    let store = opened(memory, u64::MAX);
    let config = RenderConfig::seconds(RATE, SECONDS);
    let held = now(render_over(graph, target, config, &store)).expect("a render");
    now(store.persist()).expect("persisted");
    held
}

fn stats(render: &Render) -> &CacheStats {
    render.cache_stats.as_ref().expect("a render over a store")
}

fn bits(render: &Render) -> Vec<Vec<u64>> {
    let root = render.output(render.root).expect("the root's samples");
    let planes = root.planes.iter();
    planes
        .map(|p| p.iter().map(|v| v.to_bits()).collect())
        .collect()
}

fn file(body: &str) -> String {
    format!("{HEADER}{body}\n")
}

/// `x` and `y` under a sum `p`, which a master reading it at `gain` samples.
fn mixed(name: &str, gain: f64) -> Graph {
    graph_of(
        name,
        &[
            ("x", &file("sample(crop(sin(2*pi*220*t), 0s, 0.05s))")),
            ("y", &file("sample(crop(sin(2*pi*330*t), 0s, 0.05s))")),
            ("p", &file("@x*0.5 + @y")),
            ("master", &file(&format!("sample(@p*{gain})"))),
        ],
    )
}

/// A node the store answers stands under the identity it was computed under, so every reader
/// above it is named, and computes, as a cold run's does.
#[test]
fn a_warm_run_names_every_value_as_the_cold_run_did() {
    let memory = Memory::default();
    let cold = rendered(&mixed("warm-names", 0.5), "master", &memory);
    let warm = rendered(&mixed("warm-names-reader", 0.25), "master", &memory);
    let stats = stats(&warm);
    let hit = |l: &&sva_engine::Lookup| l.node == "p" && l.outcome == Outcome::Hit;
    assert!(stats.lookups.iter().any(|l| hit(&l)), "{stats:?}");
    for path in ["x", "y", "p"] {
        let (c, w) = (cold.id(path).expect("cold"), warm.id(path).expect("warm"));
        assert_eq!(
            identity(&cold.tys, c).expect("named"),
            identity(&warm.tys, w).expect("named"),
            "`{path}` is one value warm and cold"
        );
    }
    let fresh = rendered(
        &mixed("warm-names-cold", 0.25),
        "master",
        &Memory::default(),
    );
    assert_eq!(
        identity(&fresh.tys, fresh.root).expect("named"),
        identity(&warm.tys, warm.root).expect("named"),
        "the reader above a hit is the value a cold run names"
    );
    assert_eq!(bits(&fresh), bits(&warm));
}
