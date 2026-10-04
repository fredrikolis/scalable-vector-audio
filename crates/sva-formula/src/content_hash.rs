// Concern: the SipHash-1-3-128 every content address in this workspace is built from, one key per domain | Non-concern: what any one address keys | IO: (domain, words) -> Hash

use crate::hash::Hash;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HashDomain {
    WrittenClosedForm,
    ClosedFormInFrequency,
    ReadTime,
    NodeIdentity,
    CacheAddress,
    GainBoundKey,
    ProfileName,
}

impl HashDomain {
    fn key(self) -> (u64, u64) {
        (DOMAIN_KEY, self as u64)
    }
}

const DOMAIN_KEY: u64 = 0x7264_6461_2061_7673;

/// SipHash-1-3-128 over whole words: taken as a random function, `n` inputs in one domain collide
/// with odds at most `n(n-1)/2^129`, against accident only, as its keys are public.
#[derive(Clone)]
pub struct ContentHasher {
    v: [u64; 4],
    words: u64,
}

impl ContentHasher {
    pub fn new(domain: HashDomain) -> ContentHasher {
        let (k0, k1) = domain.key();
        ContentHasher {
            v: [
                k0 ^ 0x736f_6d65_7073_6575,
                k1 ^ 0x646f_7261_6e64_6f6d ^ 0xee,
                k0 ^ 0x6c79_6765_6e65_7261,
                k1 ^ 0x7465_6462_7974_6573,
            ],
            words: 0,
        }
    }

    pub fn word(&mut self, word: u64) {
        self.compress(word);
        self.words += 1;
    }

    pub fn hash(&mut self, held: Hash) {
        self.word(held.0);
        self.word(held.1);
    }

    pub fn text(&mut self, text: &str) {
        self.word(text.len() as u64);
        for chunk in text.as_bytes().chunks(8) {
            let mut bytes = [0; 8];
            bytes[..chunk.len()].copy_from_slice(chunk);
            self.word(u64::from_le_bytes(bytes));
        }
    }

    pub fn finish(mut self) -> Hash {
        let length = (self.words.wrapping_mul(8) & 0xff) << 56;
        self.compress(length);
        self.v[2] ^= 0xee;
        self.rounds(3);
        let low = self.v.iter().fold(0, |held, v| held ^ v);
        self.v[1] ^= 0xdd;
        self.rounds(3);
        let high = self.v.iter().fold(0, |held, v| held ^ v);
        Hash(low, high)
    }

    fn compress(&mut self, m: u64) {
        self.v[3] ^= m;
        self.rounds(1);
        self.v[0] ^= m;
    }

    fn rounds(&mut self, n: usize) {
        let [v0, v1, v2, v3] = &mut self.v;
        for _ in 0..n {
            *v0 = v0.wrapping_add(*v1);
            *v1 = v1.rotate_left(13) ^ *v0;
            *v0 = v0.rotate_left(32);
            *v2 = v2.wrapping_add(*v3);
            *v3 = v3.rotate_left(16) ^ *v2;
            *v0 = v0.wrapping_add(*v3);
            *v3 = v3.rotate_left(21) ^ *v0;
            *v2 = v2.wrapping_add(*v1);
            *v1 = v1.rotate_left(17) ^ *v2;
            *v2 = v2.rotate_left(32);
        }
    }
}
