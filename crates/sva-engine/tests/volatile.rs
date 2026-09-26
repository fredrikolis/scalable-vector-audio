// Concern: proves what a moving parameter reaches keeps one value per node, however far it moves | Non-concern: the store's cap or prunes (stores.rs) | IO: (a composition, names) -> CacheStats

mod fixtures;

use std::collections::BTreeMap;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Cache, CacheStats, Outcome, PayloadKind, Render, RenderConfig, render};

const SECONDS: f64 = 0.05;
const RATE: u32 = 8_000;
const FX: &str = "tone(";

/// A note through a tone knob, and a send that never reads the knob.
fn mix(cutoff: f64) -> Graph {
    graph_of(
        "volatile",
        &[
            (
                "note",
                "sample(sin(2*pi*220*t) + 0.3*sin(2*pi*660*t))*0.5\n",
            ),
            ("tone", "lowpass(x, cutoff=cutoff, q=0.7)\n"),
            ("send", "lowpass(x, cutoff=3000, q=0.5)*0.2\n"),
            (
                "master",
                &format!("@tone(t, x=@note, cutoff={cutoff}) + @send(t, x=@note)\n"),
            ),
        ],
    )
}

fn run(graph: &Graph, volatile: &[&str], cache: &Cache) -> Result<Render, sva_engine::EngineError> {
    let mut config = RenderConfig::seconds(RATE, SECONDS);
    config.volatile = volatile.iter().map(|n| (*n).to_string()).collect();
    render(graph, "master", config, Some(cache))
}

fn played(cutoff: f64, volatile: &[&str], cache: &Cache) -> CacheStats {
    run(&mix(cutoff), volatile, cache)
        .unwrap_or_else(|e| panic!("rendering at {cutoff}: {e}"))
        .cache_stats
        .expect("a render handed a store reports on it")
}

fn samples(r: &Render) -> BTreeMap<String, Vec<f64>> {
    r.buffers
        .iter()
        .map(|(id, b)| (r.tys.name(*id).to_string(), b.plane(0).to_vec()))
        .collect()
}

/// A render's buffer lookups split by whether the knob moved their key off one it asked before:
/// the moved ones are what the knob reaches, the rest are shared with the render before it.
fn reach(before: &CacheStats, after: &CacheStats) -> (Vec<Outcome>, Vec<Outcome>) {
    let (moved, kept): (Vec<_>, Vec<_>) = after
        .lookups
        .iter()
        .filter(|l| l.kind == PayloadKind::Samples)
        .partition(|l| !before.lookups.iter().any(|b| b.key == l.key));
    let outcomes = |set: Vec<&sva_engine::Lookup>| set.iter().map(|l| l.outcome).collect();
    (outcomes(moved), outcomes(kept))
}

fn all(outcomes: &[Outcome], want: Outcome) -> bool {
    !outcomes.is_empty() && outcomes.iter().all(|o| *o == want)
}

#[test]
fn a_moving_knob_hits_the_note_and_replaces_the_fx_in_place() {
    let store = Cache::new();
    let warm = played(400.0, &[], &store);

    let moved = played(800.0, &["cutoff"], &store);
    let (fx, shared) = reach(&warm, &moved);
    assert!(all(&shared, Outcome::Hit), "{moved:?}");
    assert!(all(&fx, Outcome::ComputedStored), "{moved:?}");
    let entries = store.entries();

    let again = played(800.0, &["cutoff"], &store);
    let (fx, shared) = reach(&warm, &again);
    assert!(all(&fx, Outcome::Hit), "{again:?}");
    assert!(all(&shared, Outcome::Hit));

    let next = played(1200.0, &["cutoff"], &store);
    let (fx, shared) = reach(&warm, &next);
    assert!(all(&fx, Outcome::ComputedReplaced), "{next:?}");
    assert!(all(&shared, Outcome::Hit));
    assert_eq!(next.replaced(), fx.len());
    assert_eq!(
        store.entries(),
        entries,
        "one value per node, however far the knob moves"
    );
}

#[test]
fn a_volatile_render_sounds_exactly_as_a_plain_one() {
    for cutoff in [300.0, 1500.0] {
        let store = Cache::new();
        let volatile = run(&mix(cutoff), &["cutoff"], &store).expect("a render");
        let plain = run(&mix(cutoff), &[], &Cache::new()).expect("a render");
        assert_eq!(samples(&volatile), samples(&plain), "at {cutoff}");
        let answered = run(&mix(cutoff), &["cutoff"], &store).expect("a render");
        assert_eq!(
            samples(&answered),
            samples(&plain),
            "and from the store at {cutoff}"
        );
    }
}

#[test]
fn a_name_the_target_binds_nowhere_is_refused() {
    let Err(refused) = run(&mix(400.0), &["cutof"], &Cache::new()) else {
        panic!("a misspelled knob renders nothing")
    };
    assert_eq!(refused.code(), "render.volatile_unbound");
    assert!(refused.to_string().contains("cutof"), "{refused}");
}

/// A caller naming its own parameter moves every instance it binds through.
#[test]
fn a_knob_passed_down_under_another_name_is_still_volatile() {
    let graph = |c: f64| {
        graph_of(
            "passed",
            &[
                ("note", "sample(sin(2*pi*220*t))*0.5\n"),
                ("tone", "lowpass(x, cutoff=cutoff, q=0.7)\n"),
                ("strip", "@tone(t, x=x, cutoff=c)*0.9\n"),
                ("master", &format!("@strip(t, x=@note, c={c})\n")),
            ],
        )
    };
    let store = Cache::new();
    let played = |c: f64| {
        run(&graph(c), &["c"], &store)
            .expect("a render")
            .cache_stats
            .expect("stats")
    };
    played(400.0);
    let entries = store.entries();
    let moved = played(900.0);
    assert!(
        moved
            .lookups
            .iter()
            .any(|l| l.node.starts_with(FX) && l.outcome == Outcome::ComputedReplaced),
        "{moved:?}"
    );
    assert_eq!(moved.stored(), 0, "{moved:?}");
    assert_eq!(store.entries(), entries);
}
