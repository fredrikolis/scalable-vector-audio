// Concern: proves what a moving parameter reaches keeps one value per node, however far it moves | Non-concern: memory's cap or prunes (stores.rs) | IO: (a composition, names) -> CacheStats

use crate::fixtures::{graph_of, samples};
use sva_ast::Graph;
use sva_engine::{CacheStats, Outcome, PayloadKind, Render, RenderConfig, Tier, render};

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

fn run(graph: &Graph, volatile: &[&str], cache: &Tier) -> Result<Render, sva_engine::EngineError> {
    let mut config = RenderConfig::seconds(RATE, SECONDS);
    config.volatile = volatile.iter().map(|n| (*n).to_string()).collect();
    render(graph, "master", config, cache)
}

fn played(cutoff: f64, volatile: &[&str], cache: &Tier) -> CacheStats {
    run(&mix(cutoff), volatile, cache)
        .unwrap_or_else(|e| panic!("rendering at {cutoff}: {e}"))
        .cache_stats
        .expect("a render handed a store reports on it")
}

/// A render's buffer lookups, each a value's or a node memory answered, split by whether the
/// knob moved their key off one it asked before: the moved ones are what the knob reaches, the
/// rest are shared with the render before it.
fn reach(before: &CacheStats, after: &CacheStats) -> (Vec<Outcome>, Vec<Outcome>) {
    let (moved, kept): (Vec<_>, Vec<_>) = after
        .lookups
        .iter()
        .filter(|l| matches!(l.kind, PayloadKind::Segments | PayloadKind::Run))
        .filter(|l| l.store.is_none() || l.outcome == Outcome::Hit)
        .partition(|l| !before.lookups.iter().any(|b| b.key == l.key));
    let outcomes = |set: Vec<&sva_engine::Lookup>| set.iter().map(|l| l.outcome).collect();
    (outcomes(moved), outcomes(kept))
}

fn all(outcomes: &[Outcome], want: Outcome) -> bool {
    !outcomes.is_empty() && outcomes.iter().all(|o| *o == want)
}

#[test]
fn a_moving_knob_hits_the_note_and_replaces_the_fx_in_place() {
    let store = Tier::default();
    let warm = played(400.0, &[], &store);

    let moved = played(800.0, &["cutoff"], &store);
    let (fx, shared) = reach(&warm, &moved);
    assert!(all(&shared, Outcome::Hit), "{moved:?}");
    assert!(all(&fx, Outcome::ComputedStored), "{moved:?}");
    let entries = store.entries();

    let again = played(800.0, &["cutoff"], &store);
    let (fx, shared) = reach(&warm, &again);
    assert!(all(&fx, Outcome::Hit), "{again:?}");
    assert!(
        shared.iter().all(|o| *o == Outcome::Hit),
        "the target answers for what it holds: {again:?}"
    );

    let next = played(1200.0, &["cutoff"], &store);
    let (fx, shared) = reach(&warm, &next);
    assert!(all(&fx, Outcome::ComputedReplaced), "{next:?}");
    assert!(all(&shared, Outcome::Hit), "{next:?}");
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
        let store = Tier::default();
        let volatile = run(&mix(cutoff), &["cutoff"], &store).expect("a render");
        let plain = run(&mix(cutoff), &[], &Tier::default()).expect("a render");
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
    let Err(refused) = run(&mix(400.0), &["cutof"], &Tier::default()) else {
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
    let store = Tier::default();
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

/// A slot is what a node computes with its knob held, never its file's name: the same knob on
/// a renamed copy of the fx moves the one value it kept.
#[test]
fn a_knob_on_a_renamed_fx_moves_the_value_its_original_kept() {
    let graph = |fx: &str, cutoff: f64| {
        graph_of(
            "renamed-knob",
            &[
                ("note", "sample(sin(2*pi*220*t))*0.5\n"),
                (fx, "lowpass(x, cutoff=cutoff, q=0.7)\n"),
                ("master", &format!("@{fx}(t, x=@note, cutoff={cutoff})\n")),
            ],
        )
    };
    let store = Tier::default();
    let played = |fx: &str, cutoff: f64| {
        run(&graph(fx, cutoff), &["cutoff"], &store)
            .expect("a render")
            .cache_stats
            .expect("stats")
    };
    played("tone", 400.0);
    let entries = store.entries();
    let moved = played("fx/colour", 900.0);
    assert!(
        moved
            .lookups
            .iter()
            .any(|l| l.outcome == Outcome::ComputedReplaced),
        "{moved:?}"
    );
    assert_eq!(
        store.entries(),
        entries,
        "one value for the knob, whatever the file"
    );
}

/// A knob read where its stand-in cannot be, as a whole count of samples back, still renders;
/// its values keep an entry each, and the render says why.
#[test]
fn a_knob_its_stand_in_cannot_type_renders_and_says_why_it_keeps_each_value() {
    let graph = graph_of(
        "uncountable-knob",
        &[
            ("note", "sample(crop(sin(2*pi*220*t), 0s, 0.02s))\n"),
            ("echo", "x + 0.5*self[idx(t) - back]\n"),
            ("master", "@echo(t, x=@note, back=40)\n"),
        ],
    );
    let held = run(&graph, &["back"], &Tier::default()).expect("a render");
    let stats = held.cache_stats.expect("stats");
    assert!(stats.unslotted.is_some(), "{stats:?}");
}

/// Two instances apart only by a knob keep a value each: moving one never evicts the other,
/// and the two read in the other order are both answered.
#[test]
fn two_instances_apart_only_by_a_knob_each_keep_their_value() {
    let graph = |a: u32, b: u32| {
        graph_of(
            "siblings",
            &[
                ("note", "sample(sin(2*pi*220*t))*0.5\n"),
                ("tone", "lowpass(x, cutoff=cutoff, q=0.7)\n"),
                (
                    "master",
                    &format!("@tone(t, x=@note, cutoff={a}) + 0.5*@tone(t, x=@note, cutoff={b})\n"),
                ),
            ],
        )
    };
    let store = Tier::default();
    let played = |a: u32, b: u32| {
        run(&graph(a, b), &["cutoff"], &store)
            .expect("a render")
            .cache_stats
            .expect("stats")
    };
    played(200, 800);
    let entries = store.entries();
    let tones = |stats: &CacheStats, at: &str| -> Vec<Outcome> {
        let tone = stats.lookups.iter().filter(|l| l.node.starts_with(at));
        tone.map(|l| l.outcome).collect()
    };
    for a in [300, 400, 500] {
        let moved = played(a, 800);
        let kept = tones(&moved, "tone(cutoff=800");
        assert!(all(&kept, Outcome::Hit), "at {a}: {moved:?}");
        assert_eq!(store.entries(), entries, "at {a}");
    }
    let swapped = played(800, 500);
    assert!(all(&tones(&swapped, FX), Outcome::Hit), "{swapped:?}");
    assert_eq!(store.entries(), entries);
}
