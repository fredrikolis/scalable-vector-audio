// Concern: where this build's own f64 `exp` first returns exactly zero | Non-concern: which factor that silences (sva-samples' atom windows, sva-engine's supports) | IO: () -> f64

use std::sync::OnceLock;

/// Read off the `exp` this build links: its subnormals, not a textbook bound, decide it.
pub fn exp_zero_at() -> f64 {
    static FOUND: OnceLock<f64> = OnceLock::new();
    *FOUND.get_or_init(found)
}

const FLOOR: f64 = -4096.0;

fn found() -> f64 {
    let (mut zero, mut live) = (FLOOR.to_bits(), (-1.0f64).to_bits());
    assert!(FLOOR.exp() == 0.0, "exp underflows before {FLOOR}");
    while zero - live > 1 {
        let mid = live + (zero - live) / 2;
        match f64::from_bits(mid).exp() == 0.0 {
            true => zero = mid,
            false => live = mid,
        }
    }
    let mut edge = f64::from_bits(zero);
    while let Some(broken) = nonzero_below(edge) {
        edge = broken.next_down();
    }
    edge
}

fn nonzero_below(edge: f64) -> Option<f64> {
    const STEPS: u64 = 1 << 16;
    let near = (0..STEPS).map(|k| f64::from_bits(edge.to_bits() + k));
    let far = (0..=STEPS).map(|k| edge + (FLOOR - edge) * (k as f64 / STEPS as f64));
    near.chain(far).find(|x| x.exp() != 0.0)
}
