// Concern: proves a volatile render keeps what a moving parameter reaches in slots and nowhere else | Non-concern: a store's medium or budget (stores.rs) | IO: (a composition, names) -> CacheStats

mod fixtures;

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{
    Cache, CacheStats, Entry, Expected, Hash, MemoryCache, Outcome, Pack, Payload, PayloadKind,
    Put, Render, RenderConfig, Slots, Tier, Tiered, VecMedium, render_with_slots,
};
use sva_samples::{Buffer, FilterTrace, Label};

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

fn run(
    graph: &Graph,
    volatile: &[&str],
    cache: &dyn Cache,
    slots: &Slots,
) -> Result<Render, sva_engine::EngineError> {
    let mut config = RenderConfig::seconds(RATE, SECONDS);
    config.volatile = volatile.iter().map(|n| (*n).to_string()).collect();
    render_with_slots(graph, "master", config, Some(cache), Some(slots))
}

fn played(cutoff: f64, volatile: &[&str], cache: &dyn Cache, slots: &Slots) -> CacheStats {
    run(&mix(cutoff), volatile, cache, slots)
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

/// A memory store that says every time it was written.
#[derive(Default)]
struct Watched {
    inner: MemoryCache,
    writes: Mutex<Vec<Hash>>,
}

impl Watched {
    fn writes(&self) -> Vec<Hash> {
        self.writes.lock().expect("unpoisoned").clone()
    }
}

impl Cache for Watched {
    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        self.inner.load(key, node, expected)
    }
    fn peek(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        self.inner.peek(key, node, expected)
    }
    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        self.writes.lock().expect("unpoisoned").push(key);
        self.inner.store(key, payload, traces, label);
    }
    fn holds(&self, key: Hash) -> bool {
        self.inner.holds(key)
    }
    fn worth_storing(&self, cost: Duration, bytes: usize, kind: PayloadKind) -> bool {
        self.inner.worth_storing(cost, bytes, kind)
    }
    fn sweep(&self) {
        self.inner.sweep();
    }
    fn held_bytes(&self) -> u64 {
        self.inner.held_bytes()
    }
    fn evicted_bytes(&self) -> u64 {
        self.inner.evicted_bytes()
    }
    fn max_bytes(&self) -> u64 {
        self.inner.max_bytes()
    }
}

#[test]
fn a_moving_knob_hits_the_note_and_slots_the_fx_without_touching_the_store() {
    let store = Watched::default();
    let slots = Slots::default();
    let warm = played(400.0, &[], &store, &slots);
    store.sweep();
    let (written, held) = (store.writes(), store.held_bytes());

    let moved = played(800.0, &["cutoff"], &store, &slots);
    let (fx, shared) = reach(&warm, &moved);
    assert!(all(&shared, Outcome::Hit(Tier::Memory)), "{moved:?}");
    assert!(all(&fx, Outcome::ComputedSlotted), "{moved:?}");
    assert_eq!((moved.slotted(), moved.stored()), (fx.len(), 0));
    store.sweep();
    assert_eq!(store.writes(), written, "the store was never written");
    assert_eq!(store.held_bytes(), held, "and holds what it held");
    let slotted = slots.slots();
    assert_eq!(slotted, fx.len());

    let again = played(800.0, &["cutoff"], &store, &slots);
    let (fx, shared) = reach(&warm, &again);
    assert!(all(&fx, Outcome::Hit(Tier::Volatile)), "{again:?}");
    assert!(all(&shared, Outcome::Hit(Tier::Memory)));
    assert_eq!(again.hits_in(Tier::Volatile), fx.len());

    let next = played(1200.0, &["cutoff"], &store, &slots);
    let (fx, shared) = reach(&warm, &next);
    assert!(all(&fx, Outcome::ComputedReplaced), "{next:?}");
    assert!(all(&shared, Outcome::Hit(Tier::Memory)));
    assert_eq!(next.replaced(), fx.len());
    assert_eq!(
        slots.slots(),
        slotted,
        "one slot per node, however far the knob moves"
    );
    assert_eq!(store.writes(), written);
}

#[test]
fn a_volatile_render_sounds_exactly_as_a_plain_one() {
    for cutoff in [300.0, 1500.0] {
        let slots = Slots::default();
        let volatile =
            run(&mix(cutoff), &["cutoff"], &MemoryCache::new(), &slots).expect("a render");
        let plain = run(&mix(cutoff), &[], &MemoryCache::new(), &slots).expect("a render");
        assert_eq!(samples(&volatile), samples(&plain), "at {cutoff}");
        let answered =
            run(&mix(cutoff), &["cutoff"], &MemoryCache::new(), &slots).expect("a render");
        assert_eq!(
            samples(&answered),
            samples(&plain),
            "and from its slot at {cutoff}"
        );
    }
}

#[test]
fn a_cold_note_in_a_volatile_render_is_stored_as_ever() {
    let plain = played(500.0, &[], &MemoryCache::new(), &Slots::default());
    let volatile = played(500.0, &["cutoff"], &MemoryCache::new(), &Slots::default());
    let (fx, _) = reach(
        &played(900.0, &[], &MemoryCache::new(), &Slots::default()),
        &volatile,
    );
    assert!(all(&fx, Outcome::ComputedSlotted), "{volatile:?}");
    assert_eq!(volatile.stored() + volatile.slotted(), plain.stored());
    assert!(
        volatile
            .lookups
            .iter()
            .filter(|l| l.outcome == Outcome::ComputedStored)
            .all(|l| !l.node.starts_with(FX)),
        "{volatile:?}"
    );
}

#[test]
fn a_persistent_hit_in_a_volatile_render_is_not_promoted() {
    let first = Tiered::new(
        MemoryCache::new(),
        Pack::open(VecMedium::default(), 1 << 30),
    );
    let warm = played(600.0, &[], &first, &Slots::default());
    first.sweep();
    let reopened = Tiered::new(
        MemoryCache::new(),
        Pack::open(VecMedium::holding(first.back.medium().bytes()), 1 << 30),
    );
    let stats = played(600.0, &["cutoff"], &reopened, &Slots::default());
    let other = played(900.0, &[], &MemoryCache::new(), &Slots::default());
    let fx: Vec<_> = stats
        .lookups
        .iter()
        .filter(|l| l.kind == PayloadKind::Samples)
        .filter(|l| !other.lookups.iter().any(|o| o.key == l.key))
        .collect();
    assert!(!fx.is_empty());
    for lookup in fx {
        assert_eq!(lookup.outcome, Outcome::Hit(Tier::Persistent), "{lookup:?}");
        assert!(!reopened.front.holds(lookup.key), "peeked, never promoted");
    }
    let buffers = |s: &CacheStats| {
        s.lookups
            .iter()
            .filter(|l| l.kind == PayloadKind::Samples)
            .count()
    };
    assert_eq!(buffers(&stats), buffers(&warm));
    assert_eq!(
        stats.hits(),
        buffers(&stats),
        "a pack holds no spectral sum: {stats:?}"
    );
}

#[test]
fn a_name_the_target_binds_nowhere_is_refused() {
    let Err(refused) = run(
        &mix(400.0),
        &["cutof"],
        &MemoryCache::new(),
        &Slots::default(),
    ) else {
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
    let store = MemoryCache::new();
    let slots = Slots::default();
    let played = |c: f64| {
        run(&graph(c), &["c"], &store, &slots)
            .expect("a render")
            .cache_stats
            .expect("stats")
    };
    let first = played(400.0);
    let stored: Vec<&str> = first
        .lookups
        .iter()
        .filter(|l| l.outcome == Outcome::ComputedStored)
        .map(|l| l.node.as_str())
        .collect();
    assert!(stored.iter().all(|n| *n == "note"), "{first:?}");
    let slotted = slots.slots();
    assert!(
        first
            .lookups
            .iter()
            .any(|l| l.node.starts_with(FX) && l.outcome == Outcome::ComputedSlotted)
    );

    let moved = played(900.0);
    assert_eq!(moved.stored(), 0, "{moved:?}");
    assert!(
        moved
            .lookups
            .iter()
            .any(|l| l.node.starts_with(FX) && l.outcome == Outcome::ComputedReplaced)
    );
    assert_eq!(
        moved.replaced() + moved.hits_in(Tier::Volatile),
        slotted,
        "a slot whose value the knob does not move answers again: {moved:?}"
    );
    assert_eq!(slots.slots(), slotted);
}

fn buffer(len: usize) -> Payload {
    Payload::Samples(Box::new(Buffer::of_planes(RATE, vec![vec![0.25; len]])))
}

const ONE: Expected = Expected::Samples {
    rate: RATE,
    width: 1,
    samples: 100,
};

#[test]
fn a_full_slot_store_drops_the_least_recently_heard_slot_whole() {
    let each = buffer(100).bytes() as u64;
    let slots = Slots::holding(2 * each);
    let (a, b, c) = (Hash(1, 0), Hash(2, 0), Hash(3, 0));
    let key = |n: u64| Hash(0, n);
    assert_eq!(slots.put(a, key(1), &buffer(100), &[], None), Put::Slotted);
    assert_eq!(slots.put(b, key(2), &buffer(100), &[], None), Put::Slotted);
    assert!(slots.get(a, key(1), "a", ONE).is_some(), "a is heard again");
    assert_eq!(slots.put(c, key(3), &buffer(100), &[], None), Put::Slotted);
    assert!(
        slots.get(b, key(2), "b", ONE).is_none(),
        "b went, the oldest"
    );
    assert!(slots.get(a, key(1), "a", ONE).is_some());
    assert!(slots.get(c, key(3), "c", ONE).is_some());
    assert_eq!(slots.held_bytes(), 2 * each);

    assert!(
        slots.get(a, key(9), "a", ONE).is_none(),
        "a slot answers its own key only"
    );
    assert_eq!(slots.put(a, key(9), &buffer(100), &[], None), Put::Replaced);
    assert_eq!(slots.slots(), 2);
    assert_eq!(
        slots.put(a, key(10), &buffer(1000), &[], None),
        Put::Refused
    );
    assert!(
        slots.get(a, key(9), "a", ONE).is_some(),
        "a refused value leaves the old one"
    );

    slots.bound(each);
    assert_eq!((slots.slots(), slots.held_bytes()), (1, each));
    slots.clear();
    assert_eq!((slots.slots(), slots.held_bytes()), (0, 0));
}

#[test]
fn a_small_slot_store_never_evicts_what_the_store_holds() {
    let store = MemoryCache::new();
    let warm = played(400.0, &[], &store, &Slots::default());
    let held: Vec<Hash> = warm
        .lookups
        .iter()
        .filter(|l| l.kind == PayloadKind::Samples)
        .map(|l| l.key)
        .collect();
    let tiny = Slots::holding(1);
    let stats = played(700.0, &["cutoff"], &store, &tiny);
    assert_eq!(tiny.slots(), 0, "nothing fits");
    let (fx, _) = reach(&warm, &stats);
    assert!(all(&fx, Outcome::ComputedNotStored), "{stats:?}");
    assert!(held.iter().all(|k| store.holds(*k)));
}
