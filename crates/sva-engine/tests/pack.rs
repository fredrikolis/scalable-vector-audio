// Concern: proves a pack reopens to the entries it was written, cuts a damaged tail and sweeps to its cap | Non-concern: what a key stands for (cache.rs) | IO: (Hash, a medium) -> a buffer + label

mod fixtures;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use fixtures::{Relabelled, graph_of};
use sva_engine::{
    Cache, Expected, Hash, Medium, MemoryCache, Pack, Payload, Tier, Tiered, VecMedium,
};
use sva_samples::{AutomationFrame, Buffer, FilterTrace, Label, PSYCHOACOUSTIC_V1, Rule, Source};

const READ: Expected = Expected::Samples {
    rate: 8_000,
    width: 1,
    samples: 64,
};

fn tone(seed: u64) -> Payload {
    Payload::Samples(Box::new(Buffer::mono(
        8_000,
        (0..64)
            .map(|i| (i as f64 * 0.37 + seed as f64).sin())
            .collect(),
    )))
}

fn label() -> Label {
    Label::new(
        Source::Measured,
        PSYCHOACOUSTIC_V1.name,
        8_000,
        sva_samples::Detail::Point {
            rule: Rule::PointSampled,
            alias_db: Some(-96.5),
        },
    )
}

fn trace() -> FilterTrace {
    FilterTrace {
        node: "written-by".to_string(),
        site: 1,
        channel: Some(0),
        shape: "lowpass",
        clamped: false,
        trace_secs: 0.25,
        frames: vec![AutomationFrame {
            t_secs: 0.0,
            cutoff: 900.0,
            q: 0.8,
            gain_db: 0.0,
        }],
    }
}

/// A medium that refuses writes reaching past `writable`, leaving the part before it torn in
/// place, and that fails reads and truncation on demand.
struct Flaky {
    inner: VecMedium,
    writable: AtomicU64,
    truncates: AtomicBool,
    reads: AtomicBool,
    flushes: AtomicU64,
}

impl Flaky {
    fn new() -> Flaky {
        Flaky {
            inner: VecMedium::default(),
            writable: AtomicU64::new(u64::MAX),
            truncates: AtomicBool::new(true),
            reads: AtomicBool::new(true),
            flushes: AtomicU64::new(0),
        }
    }
}

impl Medium for Flaky {
    fn size(&self) -> u64 {
        self.inner.size()
    }
    fn read_at(&self, off: u64, buf: &mut [u8]) -> bool {
        self.reads.load(Ordering::Relaxed) && self.inner.read_at(off, buf)
    }
    fn write_at(&self, off: u64, bytes: &[u8]) -> bool {
        let limit = self.writable.load(Ordering::Relaxed);
        let fits = limit.saturating_sub(off).min(bytes.len() as u64) as usize;
        self.inner.write_at(off, &bytes[..fits]) && fits == bytes.len()
    }
    fn truncate(&self, len: u64) -> bool {
        self.truncates.load(Ordering::Relaxed) && self.inner.truncate(len)
    }
    fn flush(&self) -> bool {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        true
    }
}

fn reopened(pack: &Pack<VecMedium>, max_bytes: u64) -> Pack<VecMedium> {
    Pack::open(VecMedium::holding(pack.medium().bytes()), max_bytes)
}

#[test]
fn a_reopened_pack_answers_the_bytes_label_and_traces_it_was_written() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 2), &tone(1), &[trace()], Some(&label()));
    pack.store(Hash(3, 4), &tone(3), &[], None);

    let again = reopened(&pack, u64::MAX);
    let entry = again.load(Hash(1, 2), "asked-as", READ).expect("a hit");
    assert_eq!(entry.payload, tone(1), "bit for bit");
    assert_eq!(entry.label, Some(label()));
    assert_eq!(entry.traces[0].node, "asked-as", "the reader names it");
    assert_eq!(entry.traces[0].frames, trace().frames);
    assert_eq!(entry.tier, Tier::Persistent);
    assert_eq!(
        again.load(Hash(3, 4), "n", READ).map(|e| e.payload),
        Some(tone(3))
    );
    assert_eq!(again.faults(), 0);
}

#[test]
fn the_later_of_two_records_under_one_key_wins() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], None);
    pack.store(Hash(1, 1), &tone(2), &[], None);
    let again = reopened(&pack, u64::MAX);
    assert_eq!(
        again.load(Hash(1, 1), "n", READ).map(|e| e.payload),
        Some(tone(2))
    );
}

#[test]
fn a_torn_tail_is_a_miss_and_is_cut_off() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], None);
    let whole = pack.medium().size();
    pack.store(Hash(2, 2), &tone(2), &[], None);
    let mut bytes = pack.medium().bytes();
    bytes.truncate(bytes.len() - 3);

    let again = Pack::open(VecMedium::holding(bytes), u64::MAX);
    assert!(
        again.load(Hash(1, 1), "n", READ).is_some(),
        "the whole record"
    );
    assert!(again.load(Hash(2, 2), "n", READ).is_none(), "the torn one");
    assert_eq!(
        again.medium().size(),
        whole,
        "cut back to the last whole record"
    );
    assert_eq!(again.held_bytes(), whole);
}

#[test]
fn a_flipped_byte_ends_the_pack_at_the_record_it_landed_in() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], None);
    let first = pack.medium().size() as usize;
    pack.store(Hash(2, 2), &tone(2), &[], None);
    pack.store(Hash(3, 3), &tone(3), &[], None);
    let mut bytes = pack.medium().bytes();
    bytes[first + (first / 2)] ^= 0x10;

    let again = Pack::open(VecMedium::holding(bytes), u64::MAX);
    assert!(again.load(Hash(1, 1), "n", READ).is_some());
    for key in [Hash(2, 2), Hash(3, 3)] {
        assert!(again.load(key, "n", READ).is_none(), "{key}: at or past it");
        assert!(!again.holds(key));
    }
    assert_eq!(again.medium().size() as usize, first);
}

#[test]
fn a_sweep_over_the_cap_keeps_the_most_recently_read_and_compacts() {
    let keys: Vec<Hash> = (0..8).map(|i| Hash(i, 0)).collect();
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(keys[0], &tone(0), &[], None);
    let record = pack.medium().size();

    let capped = Pack::open(VecMedium::default(), record * 5);
    for (i, &key) in keys.iter().enumerate() {
        capped.store(key, &tone(i as u64), &[], None);
    }
    for &key in &keys[4..] {
        assert!(capped.load(key, "n", READ).is_some());
    }
    capped.sweep();
    assert!(capped.held_bytes() <= capped.max_bytes());
    assert_eq!(
        capped.held_bytes(),
        capped.medium().size(),
        "the medium shrank"
    );
    assert!(capped.evicted_bytes() > 0);
    for &key in &keys[..4] {
        assert!(!capped.holds(key), "the least recently read went first");
    }

    let again = reopened(&capped, record * 5);
    for (i, &key) in keys.iter().enumerate().skip(4) {
        assert_eq!(
            again.load(key, "n", READ).map(|e| e.payload),
            Some(tone(i as u64)),
            "a survivor moved and still reads whole"
        );
    }
}

#[test]
fn a_sweep_under_the_cap_drops_nothing() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], None);
    pack.sweep();
    assert_eq!(pack.evicted_bytes(), 0);
    assert_eq!(pack.held_bytes(), pack.medium().size());
}

/// Another codec's entry is sound: it is not this store's to read, and not its to delete.
#[test]
fn an_entry_another_codec_wrote_is_a_miss_and_not_a_fault() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], None);

    let other = reopened(&pack, u64::MAX).coded(Box::new(Relabelled));
    assert!(other.load(Hash(1, 1), "n", READ).is_none());
    assert_eq!(other.faults(), 0);
    assert!(other.holds(Hash(1, 1)), "kept for the codec that wrote it");

    assert!(
        reopened(&pack, u64::MAX)
            .load(Hash(1, 1), "n", READ)
            .is_some()
    );
}

#[test]
fn only_samples_are_offered_a_record_whatever_they_cost() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    for (kind, kept) in [
        (sva_engine::PayloadKind::Samples, true),
        (sva_engine::PayloadKind::Frames, false),
        (sva_engine::PayloadKind::Symbolic, false),
    ] {
        assert_eq!(
            pack.worth_storing(std::time::Duration::ZERO, 1 << 20, kind),
            kept,
            "{kind:?}"
        );
    }
}

#[test]
fn a_tiered_store_promotes_a_persistent_hit_into_memory() {
    let pack = Pack::open(VecMedium::default(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], Some(&label()));

    let tiered = Tiered::new(MemoryCache::new(), reopened(&pack, u64::MAX));
    let first = tiered.load(Hash(1, 1), "n", READ).expect("a hit");
    assert_eq!(first.tier, Tier::Persistent);
    let second = tiered.load(Hash(1, 1), "n", READ).expect("a hit");
    assert_eq!(second.tier, Tier::Memory, "promoted");
    assert_eq!(first.payload, second.payload);
    assert_eq!(first.label, second.label);

    tiered.store(Hash(2, 2), &tone(2), &[], None);
    assert!(tiered.front.holds(Hash(2, 2)) && tiered.back.holds(Hash(2, 2)));
}

/// A write the medium refused leaves no entry behind, is counted, and does not stop the next
/// store from landing whole, whether or not the torn bytes could be cut away.
#[test]
fn a_refused_write_is_a_counted_miss_and_the_pack_keeps_working() {
    for cuts in [true, false] {
        let flaky = Flaky::new();
        flaky.truncates.store(cuts, Ordering::Relaxed);
        let pack = Pack::open(flaky, u64::MAX);
        pack.store(Hash(1, 1), &tone(1), &[], None);
        let whole = pack.medium().size();
        pack.medium()
            .writable
            .store(whole + whole / 2, Ordering::Relaxed);

        pack.store(Hash(2, 2), &tone(2), &[], None);
        assert!(pack.load(Hash(2, 2), "n", READ).is_none(), "cuts {cuts}");
        assert!(!pack.holds(Hash(2, 2)));
        assert_eq!(
            pack.faults(),
            1,
            "cuts {cuts}: the refusal is on the record"
        );
        assert_eq!(pack.held_bytes(), whole, "cuts {cuts}");

        pack.medium().writable.store(u64::MAX, Ordering::Relaxed);
        pack.store(
            Hash(3, 3),
            &Payload::Samples(Box::new(Buffer::mono(8_000, vec![0.5; 64]))),
            &[],
            None,
        );
        assert!(pack.load(Hash(3, 3), "n", READ).is_some(), "cuts {cuts}");
        assert!(pack.load(Hash(1, 1), "n", READ).is_some(), "cuts {cuts}");

        let again = Pack::open(VecMedium::holding(pack.medium().inner.bytes()), u64::MAX);
        assert!(again.load(Hash(1, 1), "n", READ).is_some(), "cuts {cuts}");
        assert!(again.load(Hash(3, 3), "n", READ).is_some(), "cuts {cuts}");
        assert!(
            !again.holds(Hash(2, 2)),
            "cuts {cuts}: only whole records scan"
        );
        assert_eq!(
            again.medium().size(),
            whole * 2,
            "cuts {cuts}: any torn tail is cut"
        );
    }
}

#[test]
fn a_refused_read_is_a_counted_miss() {
    let pack = Pack::open(Flaky::new(), u64::MAX);
    pack.store(Hash(1, 1), &tone(1), &[], None);
    pack.medium().reads.store(false, Ordering::Relaxed);
    assert!(pack.load(Hash(1, 1), "n", READ).is_none());
    assert_eq!(pack.faults(), 1);
}

/// A flush is the dear call on a browser's file system, so a render answered wholly from the
/// store, and the sweep after it, leave the medium untouched.
#[test]
fn a_render_of_nothing_but_hits_flushes_nothing() {
    let graph = graph_of(
        "flushes",
        &[
            ("osc", "saw(110*t)\n"),
            ("master", "lowpass(sample(@osc), cutoff=900, q=0.8)*0.5\n"),
        ],
    );
    let tiered = Tiered::new(MemoryCache::new(), Pack::open(Flaky::new(), u64::MAX));
    let rendered = |tiered: &Tiered<Flaky>| {
        let held = sva_engine::render(
            &graph,
            "master",
            sva_engine::RenderConfig::seconds(8_000, 0.05),
            Some(tiered),
        )
        .expect("a render");
        tiered.sweep();
        held.cache_stats.expect("stats")
    };
    let flushes = |tiered: &Tiered<Flaky>| tiered.back.medium().flushes.load(Ordering::Relaxed);

    let cold = rendered(&tiered);
    assert!(cold.stored() > 0);
    assert_eq!(
        flushes(&tiered),
        1,
        "what the cold render wrote is flushed once"
    );

    let warm = rendered(&tiered);
    assert_eq!(warm.computed(), 0, "the second render is all hits");
    assert_eq!(flushes(&tiered), 1, "and flushes nothing");
    tiered.sweep();
    assert_eq!(flushes(&tiered), 1, "nor does a bare sweep");
}
