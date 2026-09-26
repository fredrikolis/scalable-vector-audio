// Concern: proves the store answers one key with the bytes stored under it, and sweep to their budget | Non-concern: what a key stands for (cache.rs) | IO: (Hash) -> a buffer + traces

use sva_engine::{Cache, Expected, Hash, MemoryCache, Payload};
use sva_samples::{AutomationFrame, Buffer, FilterTrace};

const FOUR: Expected = Expected::Samples {
    rate: 44_100,
    width: 1,
    samples: 4,
};

fn holding(buffer: &Buffer) -> Payload {
    Payload::Samples(Box::new(buffer.clone()))
}

fn read_at(rate: u32, samples: usize) -> Expected {
    Expected::Samples {
        rate,
        width: 1,
        samples,
    }
}

fn trace() -> FilterTrace {
    FilterTrace {
        node: "written-by".to_string(),
        site: 2,
        channel: None,
        shape: "lowpass",
        clamped: true,
        trace_secs: 0.001,
        frames: vec![AutomationFrame {
            t_secs: 0.5,
            cutoff: 812.5,
            q: 3.2,
            gain_db: -1.0,
        }],
    }
}

fn odd_values() -> Buffer {
    Buffer::mono(44100, vec![0.0, -0.5, f64::MIN_POSITIVE, 0.9999999])
}

fn stores() -> Vec<(&'static str, Box<dyn Cache>)> {
    vec![("memory", Box::new(MemoryCache::new()))]
}

#[test]
fn a_stored_buffer_reads_back_bit_for_bit_under_the_readers_own_node_name() {
    let key = Hash(7, 11);
    for (name, cache) in stores() {
        cache.store(key, &holding(&odd_values()), &[trace()], None);
        assert!(cache.holds(key), "{name}");
        let entry = cache.load(key, "asked-as", FOUR).expect("a hit");
        assert_eq!(entry.payload, holding(&odd_values()), "{name}");
        assert_eq!(
            entry.traces[0].node, "asked-as",
            "{name}: the reader names it"
        );
        assert_eq!(entry.traces[0].site, 2, "{name}");
        assert_eq!(entry.traces[0].frames, trace().frames, "{name}");
    }
}

/// An entry that cannot answer its own key is garbage, truncated or merely mismatched:
/// report a miss and drop it rather than keep failing.
#[test]
fn an_entry_that_does_not_answer_what_was_asked_is_a_miss_and_is_dropped() {
    let key = Hash(7, 11);
    for (name, cache) in stores() {
        assert!(
            cache.load(key, "n", FOUR).is_none(),
            "{name}: nothing stored"
        );
        let mono = Buffer::mono(8000, vec![0.25; 4]);
        for (rate, samples, why) in [(44_100, 5, "wrong length"), (48_000, 4, "wrong rate")] {
            cache.store(key, &holding(&mono), &[], None);
            assert!(
                cache.load(key, "n", read_at(rate, samples)).is_none(),
                "{name}: {why}"
            );
            assert!(!cache.holds(key), "{name}: {why}, and cleared");
        }
    }
}

#[test]
fn a_memory_sweep_drops_the_least_recently_read_until_it_is_under_the_cap() {
    let keys: Vec<Hash> = (0..8).map(|i| Hash(i, 0)).collect();
    let read = read_at(8_000, 256);
    let fill = |cache: &MemoryCache| {
        for &key in &keys {
            cache.store(
                key,
                &holding(&Buffer::mono(8_000, vec![0.5; 256])),
                &[],
                None,
            );
        }
        for &key in &keys[4..] {
            assert!(cache.load(key, "n", read).is_some());
        }
    };

    let cache = MemoryCache::holding(u64::MAX);
    fill(&cache);
    cache.sweep();
    let whole = cache.held_bytes();
    assert_eq!(whole, 8 * 256 * 8);
    assert_eq!(cache.evicted_bytes(), 0, "nothing to drop under the cap");

    let capped = MemoryCache::holding(whole / 2);
    fill(&capped);
    capped.sweep();
    assert!(capped.held_bytes() <= capped.max_bytes());
    assert!(capped.evicted_bytes() >= whole - capped.held_bytes());
    for &key in &keys[..4] {
        assert!(!capped.holds(key), "the least recently read went first");
    }
}
