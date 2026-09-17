// Concern: prices one cached buffer's store-and-load round trip in nanoseconds a byte | Non-concern: whether storing it was worth it (cache/disk.rs owns that gate) | IO: () -> ns/byte per size

//! A warm page cache, one process: the fast case. `IO_NANOS_PER_BYTE` sits above it, and
//! `MARGIN` is the room a shared CI runner gets — a wrong price, not a busy machine.

use std::time::{Duration, Instant};

use sva_engine::{Cache, DiskCache, Expected, Hash, IO_NANOS_PER_BYTE, Payload};
use sva_samples::Buffer;

const SECONDS: [u64; 3] = [1, 4, 32];
const RATE: u32 = 44_100;
const ROUNDS: u32 = 5;
const MARGIN: f64 = 3.0;

fn main() {
    let dir = std::env::temp_dir().join(format!("sva-io-price-{:x}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a fresh directory");
    let cache = DiskCache::at(&dir);

    println!("seconds  bytes       store        load         round trip");
    let mut dearer = 0usize;
    for secs in SECONDS {
        let samples = secs as usize * RATE as usize;
        let buffer = Buffer::mono(
            RATE,
            (0..samples).map(|n| (n as f64 * 1e-4).sin()).collect(),
        );
        let payload = Payload::Samples(Box::new(buffer));
        let bytes = samples * size_of::<f64>();
        let key = Hash(secs, 0x1_0f_a1);
        let expected = Expected::Samples {
            rate: RATE,
            width: 1,
            samples,
        };

        let mut stored = Duration::ZERO;
        let mut loaded = Duration::ZERO;
        for _ in 0..ROUNDS {
            let at = Instant::now();
            cache.store(key, &payload, &[], None);
            stored += at.elapsed();
            let at = Instant::now();
            let hit = cache.load(key, "priced", expected);
            loaded += at.elapsed();
            assert!(hit.is_some(), "the entry just written reads back");
        }
        let per = |total: Duration| total.as_nanos() as f64 / (bytes * ROUNDS as usize) as f64;
        let round_trip = per(stored) + per(loaded);
        println!(
            "{secs:>7}  {bytes:<10}  {:.3} ns/B  {:.3} ns/B  {round_trip:.3} ns/B",
            per(stored),
            per(loaded)
        );
        dearer += usize::from(round_trip > MARGIN * IO_NANOS_PER_BYTE as f64);
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        dearer < SECONDS.len(),
        "every round trip above cost over {MARGIN} times the {IO_NANOS_PER_BYTE} ns/B charged"
    );
    println!("the store charges {IO_NANOS_PER_BYTE} ns/B, above what it just paid");
}
