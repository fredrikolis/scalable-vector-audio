// Concern: the order and the depth a store sweeps by | Non-concern: the store itself or its cap (memory.rs) | IO: (entries, cap) -> bytes held, bytes dropped

/// What a sweep leaves behind, for the store to report.
pub(super) struct Swept {
    pub held: u64,
    pub evicted: u64,
}

/// Least-recently-read first, down to nine tenths of the cap: stopping at the cap itself would
/// evict again on the very next render. `drop_one` answers whether the entry actually went.
pub(super) fn to_cap<R: Ord, E>(
    mut entries: Vec<(R, u64, E)>,
    cap: u64,
    mut drop_one: impl FnMut(&E) -> bool,
) -> Swept {
    let found: u64 = entries.iter().map(|(_, bytes, _)| bytes).sum();
    let mut total = found;
    if total > cap {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let target = cap / 10 * 9;
        for (_, bytes, entry) in &entries {
            if total <= target {
                break;
            }
            if drop_one(entry) {
                total -= bytes;
            }
        }
    }
    Swept {
        held: total,
        evicted: found - total,
    }
}
