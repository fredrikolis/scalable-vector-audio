// Concern: measures this machine's disk-store round trip per byte against IO_NANOS_PER_BYTE | Non-concern: gating anything on it | IO: (--release run) -> one line per buffer size
//
// Wall-clock, so never a test. Run with `cargo run --release -p sva-engine --example io_price`.

use std::time::{Duration, Instant};

use sva_engine::{Cache, DiskCache, Expected, Hash, IO_NANOS_PER_BYTE, Payload};
use sva_samples::Buffer;

const RATE: u32 = 44_100;
const ROUNDS: u32 = 5;

fn main() {
    let dir = std::env::temp_dir().join(format!("sva-io-price-{}", std::process::id()));
    let cache = DiskCache::at(&dir);
    for secs in [1usize, 4, 32] {
        let samples = secs * RATE as usize;
        let buffer = Buffer::mono(
            RATE,
            (0..samples).map(|n| (n as f64 * 1e-4).sin()).collect(),
        );
        let payload = Payload::Samples(Box::new(buffer));
        let bytes = samples * size_of::<f64>();
        let key = Hash(secs as u64, 0x1_0f_a1);
        let expected = Expected::Samples {
            rate: RATE,
            width: 1,
            samples,
        };
        let mut quickest = Duration::MAX;
        for _ in 0..ROUNDS {
            let at = Instant::now();
            cache.store(key, &payload, &[], None);
            assert!(cache.load(key, "priced", expected).is_some());
            quickest = quickest.min(at.elapsed());
        }
        let per = quickest.as_nanos() as f64 / bytes as f64;
        println!("{secs:>3} s  {bytes:>10} B  {per:.2} ns/B  (priced at {IO_NANOS_PER_BYTE} ns/B)");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
