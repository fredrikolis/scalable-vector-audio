// Concern: proves both stores answer one key with the bytes stored under it, and sweep to their budget | Non-concern: what a key stands for (cache.rs) | IO: (Hash) -> a buffer + traces

mod fixtures;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fixtures::dir_of;
use sva_engine::{Cache, DiskCache, Expected, Hash, MemoryCache, Payload, PayloadKind};
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

/// The store owns how it lays a key out on disk; a test that reconstructed the layout would
/// freeze it. Exactly one entry is stored, so the only file under the directory is it.
fn one_file_under(dir: &Path) -> PathBuf {
    let mut found = Vec::new();
    let mut work = vec![dir.to_path_buf()];
    while let Some(at) = work.pop() {
        for entry in fs::read_dir(at).into_iter().flatten().flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => work.push(entry.path()),
                _ => found.push(entry.path()),
            }
        }
    }
    assert_eq!(found.len(), 1, "one entry was stored: {found:?}");
    found.remove(0)
}

fn odd_values() -> Buffer {
    Buffer::mono(44100, vec![0.0, -0.5, f64::MIN_POSITIVE, 0.9999999])
}

fn stores() -> Vec<(&'static str, Box<dyn Cache>)> {
    vec![
        (
            "disk",
            Box::new(DiskCache::at(dir_of("cache-roundtrip", &[]))) as Box<dyn Cache>,
        ),
        ("memory", Box::new(MemoryCache::new())),
    ]
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

/// A file the reader cannot parse is not a hit to fall back on: it goes, so the next render
/// writes a whole one.
#[test]
fn a_truncated_disk_entry_is_a_miss_and_is_deleted() {
    let dir = dir_of("cache-truncated", &[]);
    let cache = DiskCache::at(&dir);
    let key = Hash(7, 11);
    cache.store(
        key,
        &holding(&Buffer::mono(8_000, vec![0.25; 4])),
        &[],
        None,
    );
    let file = one_file_under(&dir);
    let bytes = fs::read(&file).expect("the entry");
    fs::write(&file, &bytes[..bytes.len() - 3]).expect("a short write");

    assert!(
        cache.load(key, "n", read_at(8_000, 4)).is_none(),
        "truncated"
    );
    assert!(!cache.holds(key), "and cleared, not left to fail forever");
}

#[test]
fn storing_on_disk_is_gated_on_computing_costing_more_than_moving() {
    let cache = DiskCache::at(dir_of("cache-policy", &[]));
    let buffer = 32 * 44100 * size_of::<f64>();
    assert!(
        !cache.worth_storing(Duration::from_millis(10), buffer, PayloadKind::Samples),
        "under I/O"
    );
    assert!(
        cache.worth_storing(Duration::from_millis(40), buffer, PayloadKind::Samples),
        "over I/O"
    );
    assert!(
        !cache.worth_storing(Duration::from_micros(900), 16, PayloadKind::Samples),
        "a tiny buffer still has to clear the floor"
    );
    assert!(
        DiskCache::at(dir_of("cache-policy-all", &[]))
            .storing_everything()
            .worth_storing(Duration::ZERO, buffer, PayloadKind::Samples)
    );
}

/// Nothing is compared against I/O in memory: a hit is a memcpy.
#[test]
fn storing_in_memory_is_never_gated() {
    assert!(MemoryCache::new().worth_storing(Duration::ZERO, 1, PayloadKind::Samples));
    assert_eq!(MemoryCache::new().dir(), None, "nowhere to point");
}

/// An evicted composition and a changed one look alike; only these figures separate them.
#[test]
fn a_sweep_reports_what_it_held_and_what_it_had_to_delete() {
    let dir = dir_of("cache-accounting", &[]);
    let filled = |cache: &dyn Cache| {
        for i in 0..8u64 {
            cache.store(
                Hash(i, i),
                &holding(&Buffer::mono(8_000, vec![0.5; 256])),
                &[],
                None,
            );
        }
    };
    let whole = DiskCache::bounded(&dir, u64::MAX);
    filled(&whole);
    whole.sweep();
    let held = whole.held_bytes();
    assert!(held > 0);
    assert_eq!(whole.evicted_bytes(), 0, "nothing to delete under the cap");

    let capped = DiskCache::bounded(&dir, held / 2);
    capped.sweep();
    assert!(capped.held_bytes() <= capped.max_bytes());
    assert!(capped.evicted_bytes() >= held - capped.held_bytes());
    assert_eq!(capped.max_bytes(), held / 2);
}

#[test]
fn a_disk_sweep_drops_the_least_recently_read_until_it_is_under_the_cap() {
    let dir = dir_of("cache-sweep", &[]);
    let keys: Vec<Hash> = (0..8).map(|i| Hash(i, 0)).collect();
    let read = read_at(8_000, 256);
    let cache = DiskCache::at(&dir);
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

    let capped = DiskCache::bounded(&dir, 4 * 1100);
    capped.sweep();
    let survivors = keys
        .iter()
        .filter(|&&k| capped.load(k, "n", read).is_some())
        .count();
    assert!(survivors <= 4, "swept below the cap, kept {survivors}");
    assert!(survivors > 0, "and did not empty itself");
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

/// The constant is the whole gate: no machine's real I/O enters it, so the same buffer is
/// stored or skipped identically wherever a render runs.
#[test]
fn the_store_prices_a_byte_at_the_chosen_constant() {
    let cache = DiskCache::at(dir_of("cache-price", &[]));
    let bytes = 1 << 20;
    let priced = Duration::from_nanos(bytes as u64 * sva_engine::IO_NANOS_PER_BYTE);
    assert!(
        !cache.worth_storing(priced, bytes, PayloadKind::Samples),
        "at the price, not over it"
    );
    assert!(cache.worth_storing(
        priced + Duration::from_nanos(1),
        bytes,
        PayloadKind::Samples
    ));
}

/// A store that has quietly stopped persisting answers every load with a miss, exactly as a
/// cold one does. The fault count is what separates the two.
#[test]
fn a_damaged_entry_is_dropped_and_counted_rather_than_read_as_a_cold_miss() {
    let dir = dir_of("cache-faults", &[]);
    let cache = DiskCache::at(&dir);
    let key = Hash(3, 5);
    cache.store(
        key,
        &holding(&Buffer::mono(8_000, vec![0.25; 4])),
        &[],
        None,
    );
    assert_eq!(cache.faults(), 0, "a clean write faults nothing");

    let file = one_file_under(&dir);
    let bytes = fs::read(&file).expect("the entry");
    fs::write(&file, &bytes[..bytes.len() - 3]).expect("a short write");

    assert!(cache.load(key, "n", read_at(8_000, 4)).is_none(), "a miss");
    assert_eq!(cache.faults(), 1, "and the reason is on the record");
    assert!(!cache.holds(key), "the damaged entry is gone");

    assert_eq!(MemoryCache::new().faults(), 0, "nothing on disk to damage");

    cache.store(
        key,
        &holding(&Buffer::mono(8_000, vec![0.25; 4])),
        &[],
        None,
    );
    let file = one_file_under(&dir);
    let shut = fs::Permissions::from_mode(0o000);
    fs::set_permissions(&file, shut).expect("an unreadable entry");
    assert!(cache.load(key, "n", read_at(8_000, 4)).is_none(), "a miss");
    assert_eq!(
        cache.faults(),
        2,
        "an entry that is there but unreadable counts"
    );
    let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
}
