// Concern: the two-lane FNV mixer every content address in this workspace is built from | Non-concern: what any one address keys (hash.rs, sva-engine's cache and refs) | IO: (words) -> Hash

use crate::hash::Hash;

/// Two independently primed FNV-1a lanes over one stream of words. `ROTATE` staggers the
/// second lane, and each caller's own value is part of its address space: change one and
/// every address that value ever produced is retired with it.
#[derive(Clone, Copy)]
pub struct Lanes<const ROTATE: u32>(u64, u64);

impl<const ROTATE: u32> Default for Lanes<ROTATE> {
    fn default() -> Lanes<ROTATE> {
        Lanes(0xcbf2_9ce4_8422_2325, 0x9e37_79b9_7f4a_7c15)
    }
}

impl<const ROTATE: u32> Lanes<ROTATE> {
    /// Continuing from an address already built, rather than from the primes.
    pub fn from(seed: Hash) -> Lanes<ROTATE> {
        Lanes(seed.0, seed.1)
    }

    pub fn word(&mut self, part: u64) {
        self.0 = (self.0 ^ part).wrapping_mul(0x0000_0100_0000_01b3);
        self.1 = (self.1 ^ part.rotate_left(ROTATE)).wrapping_mul(0x8803_55f2_1e6d_1965);
    }

    pub fn finish(self) -> Hash {
        Hash(self.0, self.1)
    }
}
