// Concern: counts the steps a per-node fold takes, where a test asks | Non-concern: what any fold computes | IO: () -> a count per thread

#[cfg(test)]
thread_local! {
    static STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// `n` more steps; nothing outside a test build.
#[inline]
pub(crate) fn step(n: usize) {
    #[cfg(test)]
    STEPS.with(|steps| steps.set(steps.get() + n as u64));
    #[cfg(not(test))]
    let _ = n;
}

#[cfg(test)]
pub(crate) fn taken() -> u64 {
    STEPS.with(std::cell::Cell::get)
}
