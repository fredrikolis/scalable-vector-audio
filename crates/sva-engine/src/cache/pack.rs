// Concern: keeps rendered buffers as records appended to one file-like medium, across sessions | Non-concern: what the medium is, one entry's bytes (entry_bytes.rs) | IO: (Hash) -> a buffer + traces

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use sva_formula::{Hash, Lanes};
use sva_samples::{FilterTrace, Label};

use super::entry_bytes::{self, RawF64, SampleCodec, Unread};
use super::evict;
use super::{Cache, Entry, Expected, Payload, PayloadKind};

/// Bumped with the record layout; a caller naming its file by it never opens an older one.
pub const FORMAT: u32 = 1;

const MAGIC: u32 = u32::from_le_bytes(*b"SVP1");
/// magic, body length, key, checksum.
const HEADER: usize = 4 + 4 + 16 + 8;
const CHECKSUM_ROTATE: u32 = 29;

/// Bytes at offsets, as one file offers them. A medium that fails answers `false`, and the pack
/// reads that as a miss or a write that did not happen.
pub trait Medium: Sync {
    fn size(&self) -> u64;

    fn read_at(&self, off: u64, buf: &mut [u8]) -> bool;

    fn write_at(&self, off: u64, bytes: &[u8]) -> bool;

    fn truncate(&self, len: u64) -> bool;

    fn flush(&self) -> bool;
}

/// A medium in this process's heap, for a caller that wants the pack's bytes themselves.
#[derive(Default)]
pub struct VecMedium(Mutex<Vec<u8>>);

impl VecMedium {
    pub fn holding(bytes: Vec<u8>) -> VecMedium {
        VecMedium(Mutex::new(bytes))
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.held().clone()
    }

    fn held(&self) -> MutexGuard<'_, Vec<u8>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Medium for VecMedium {
    fn size(&self) -> u64 {
        self.held().len() as u64
    }

    fn read_at(&self, off: u64, buf: &mut [u8]) -> bool {
        let held = self.held();
        let Some(from) = held.get(off as usize..off as usize + buf.len()) else {
            return false;
        };
        buf.copy_from_slice(from);
        true
    }

    fn write_at(&self, off: u64, bytes: &[u8]) -> bool {
        let mut held = self.held();
        let end = off as usize + bytes.len();
        if held.len() < end {
            held.resize(end, 0);
        }
        held[off as usize..end].copy_from_slice(bytes);
        true
    }

    fn truncate(&self, len: u64) -> bool {
        self.held().resize(len as usize, 0);
        true
    }

    fn flush(&self) -> bool {
        true
    }
}

#[derive(Clone, Copy)]
struct Slot {
    off: u64,
    body: u32,
    read: u64,
}

impl Slot {
    fn bytes(self) -> u64 {
        HEADER as u64 + u64::from(self.body)
    }
}

struct State {
    index: HashMap<Hash, Slot>,
    end: u64,
    clock: u64,
}

/// Records `[magic u32][len u32][key 16B][checksum u64][entry]`, appended and never rewritten
/// in place except by a sweep. Only samples are kept, as on disk. Opening reads every record,
/// the later of two under one key winning, and cuts the medium at the first that does not
/// check out: a torn tail is a miss, never a misread.
pub struct Pack<M: Medium> {
    medium: M,
    state: Mutex<State>,
    codec: Box<dyn SampleCodec>,
    max_bytes: u64,
    evicted: AtomicU64,
    faults: AtomicU64,
}

impl<M: Medium> Pack<M> {
    pub fn open(medium: M, max_bytes: u64) -> Pack<M> {
        let mut state = State {
            index: HashMap::new(),
            end: 0,
            clock: 0,
        };
        let len = medium.size();
        while let Some((key, slot)) = record_at(&medium, state.end, len, state.clock) {
            state.index.insert(key, slot);
            state.end += slot.bytes();
            state.clock += 1;
        }
        if state.end < len {
            medium.truncate(state.end);
        }
        Pack {
            medium,
            state: Mutex::new(state),
            codec: Box::new(RawF64),
            max_bytes,
            evicted: AtomicU64::new(0),
            faults: AtomicU64::new(0),
        }
    }

    pub fn coded(mut self, codec: Box<dyn SampleCodec>) -> Pack<M> {
        self.codec = codec;
        self
    }

    pub fn medium(&self) -> &M {
        &self.medium
    }

    fn locked(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fault(&self) {
        self.faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Survivors move toward the start in offset order, so each lands at or before where it
    /// was and none is overwritten before it is read. Cut short, the tail simply fails to scan.
    fn compact(&self, state: &mut State) {
        let mut order: Vec<(Hash, Slot)> = state.index.iter().map(|(k, s)| (*k, *s)).collect();
        order.sort_by_key(|(_, slot)| slot.off);
        let mut cursor = 0;
        for (key, slot) in order {
            if slot.off != cursor {
                let mut record = vec![0; slot.bytes() as usize];
                if !self.medium.read_at(slot.off, &mut record)
                    || !self.medium.write_at(cursor, &record)
                {
                    self.fault();
                    state.index.remove(&key);
                    continue;
                }
            }
            state.index.insert(
                key,
                Slot {
                    off: cursor,
                    ..slot
                },
            );
            cursor += slot.bytes();
        }
        if !self.medium.truncate(cursor) {
            self.fault();
        }
        state.end = cursor;
    }
}

fn checksum(key: Hash, body: &[u8]) -> u64 {
    let mut lanes = Lanes::<CHECKSUM_ROTATE>::default();
    lanes.word(key.0);
    lanes.word(key.1);
    lanes.word(body.len() as u64);
    for chunk in body.chunks(8) {
        let mut word = [0; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        lanes.word(u64::from_le_bytes(word));
    }
    let Hash(a, b) = lanes.finish();
    a ^ b
}

fn header(key: Hash, body: &[u8]) -> [u8; HEADER] {
    let mut out = [0; HEADER];
    out[..4].copy_from_slice(&MAGIC.to_le_bytes());
    out[4..8].copy_from_slice(&(body.len() as u32).to_le_bytes());
    out[8..16].copy_from_slice(&key.0.to_le_bytes());
    out[16..24].copy_from_slice(&key.1.to_le_bytes());
    out[24..].copy_from_slice(&checksum(key, body).to_le_bytes());
    out
}

fn word<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    bytes[at..at + N].try_into().expect("a header field")
}

/// The record at `off`, if it is whole and its checksum holds.
fn record_at(medium: &impl Medium, off: u64, len: u64, read: u64) -> Option<(Hash, Slot)> {
    let (key, body) = read_record(medium, off, len)?;
    Some((
        key,
        Slot {
            off,
            body: body.len() as u32,
            read,
        },
    ))
}

fn read_record(medium: &impl Medium, off: u64, len: u64) -> Option<(Hash, Vec<u8>)> {
    let mut head = [0; HEADER];
    if off + HEADER as u64 > len || !medium.read_at(off, &mut head) {
        return None;
    }
    if u32::from_le_bytes(word(&head, 0)) != MAGIC {
        return None;
    }
    let body_len = u32::from_le_bytes(word(&head, 4));
    if off + HEADER as u64 + u64::from(body_len) > len {
        return None;
    }
    let key = Hash(
        u64::from_le_bytes(word(&head, 8)),
        u64::from_le_bytes(word(&head, 16)),
    );
    let mut body = vec![0; body_len as usize];
    if !medium.read_at(off + HEADER as u64, &mut body) {
        return None;
    }
    (checksum(key, &body) == u64::from_le_bytes(word(&head, 24))).then_some((key, body))
}

impl<M: Medium> Cache for Pack<M> {
    fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// The medium's whole length, records a later one replaced included, until a sweep.
    fn held_bytes(&self) -> u64 {
        self.locked().end
    }

    fn evicted_bytes(&self) -> u64 {
        self.evicted.load(Ordering::Relaxed)
    }

    fn faults(&self) -> u64 {
        self.faults.load(Ordering::Relaxed)
    }

    fn holds(&self, key: Hash) -> bool {
        self.locked().index.contains_key(&key)
    }

    fn load(&self, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let Expected::Samples {
            rate,
            width,
            samples,
        } = expected
        else {
            return None;
        };
        let mut state = self.locked();
        let slot = *state.index.get(&key)?;
        let body = match read_record(&self.medium, slot.off, state.end) {
            Some((found, body)) if found == key => body,
            _ => {
                self.fault();
                state.index.remove(&key);
                return None;
            }
        };
        match entry_bytes::decode(&body, node, (rate, width, samples), self.codec.as_ref()) {
            Ok(entry) => {
                let read = state.clock;
                state.clock += 1;
                state.index.insert(key, Slot { read, ..slot });
                Some(entry)
            }
            Err(Unread::OtherCodec) => None,
            Err(Unread::Damaged) => {
                self.fault();
                state.index.remove(&key);
                None
            }
        }
    }

    /// No clock enters it: a value a browser renders has no measured cost to weigh.
    fn worth_storing(&self, _cost: Duration, _bytes: usize, kind: PayloadKind) -> bool {
        kind == PayloadKind::Samples
    }

    fn store(&self, key: Hash, payload: &Payload, traces: &[FilterTrace], label: Option<&Label>) {
        let Payload::Samples(buffer) = payload else {
            return;
        };
        let body = entry_bytes::encode(buffer, traces, label, self.codec.as_ref());
        let Ok(body_len) = u32::try_from(body.len()) else {
            self.fault();
            return;
        };
        let mut record = header(key, &body).to_vec();
        record.extend_from_slice(&body);
        let mut state = self.locked();
        let off = state.end;
        if !self.medium.write_at(off, &record) {
            self.fault();
            self.medium.truncate(off);
            return;
        }
        let read = state.clock;
        state.clock += 1;
        state.end += record.len() as u64;
        state.index.insert(
            key,
            Slot {
                off,
                body: body_len,
                read,
            },
        );
    }

    fn sweep(&self) {
        let mut state = self.locked();
        let mut evicted = 0;
        if state.end > self.max_bytes {
            let order = state
                .index
                .iter()
                .map(|(key, slot)| (slot.read, slot.bytes(), *key))
                .collect();
            let index = &mut state.index;
            evicted =
                evict::to_cap(order, self.max_bytes, |key| index.remove(key).is_some()).evicted;
            self.compact(&mut state);
        }
        self.evicted.store(evicted, Ordering::Relaxed);
        if !self.medium.flush() {
            self.fault();
        }
    }
}
