// Concern: keeps one last value per volatile node, in memory, under a byte cap | Non-concern: which nodes are volatile (render/volatile.rs), every other store | IO: (slot, key) -> a payload + traces

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use sva_formula::Hash;
use sva_samples::{FilterTrace, Label};

use super::{Entry, Expected, Payload, Tier};

pub const DEFAULT_SLOT_BYTES: u64 = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Put {
    Slotted,
    Replaced,
    /// Larger than the whole cap: the slot's old value stands.
    Refused,
}

struct Slotted {
    key: Hash,
    payload: Payload,
    traces: Vec<FilterTrace>,
    label: Option<Label>,
    read: u64,
}

impl Slotted {
    fn bytes(&self) -> u64 {
        self.payload.bytes() as u64
    }
}

struct State {
    held: HashMap<Hash, Slotted>,
    bytes: u64,
    max_bytes: u64,
    clock: u64,
}

impl State {
    fn evict(&mut self, keep: Option<Hash>) {
        while self.bytes > self.max_bytes {
            let oldest = self
                .held
                .iter()
                .filter(|(slot, _)| Some(**slot) != keep)
                .min_by_key(|(_, held)| held.read)
                .map(|(slot, _)| *slot);
            let Some(slot) = oldest else { return };
            if let Some(gone) = self.held.remove(&slot) {
                self.bytes -= gone.bytes();
            }
        }
    }
}

/// A slot is keyed coarsely, by what a node is with its volatile values left out; a hit also
/// needs the value's full key, so a coarse slot costs a miss and never a wrong buffer.
pub struct Slots {
    state: Mutex<State>,
}

impl Default for Slots {
    fn default() -> Slots {
        Slots::holding(DEFAULT_SLOT_BYTES)
    }
}

impl Slots {
    pub fn holding(max_bytes: u64) -> Slots {
        Slots {
            state: Mutex::new(State {
                held: HashMap::new(),
                bytes: 0,
                max_bytes,
                clock: 0,
            }),
        }
    }

    fn locked(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| {
            let mut state = poisoned.into_inner();
            state.held.clear();
            state.bytes = 0;
            self.state.clear_poison();
            state
        })
    }

    pub fn get(&self, slot: Hash, key: Hash, node: &str, expected: Expected) -> Option<Entry> {
        let mut state = self.locked();
        let tick = state.clock;
        state.clock += 1;
        let held = state.held.get_mut(&slot)?;
        if held.key != key || !held.payload.answers(expected) {
            return None;
        }
        held.read = tick;
        Some(Entry {
            payload: held.payload.clone(),
            traces: super::renamed(&held.traces, node),
            label: held.label.clone(),
            tier: Tier::Volatile,
        })
    }

    pub fn put(
        &self,
        slot: Hash,
        key: Hash,
        payload: &Payload,
        traces: &[FilterTrace],
        label: Option<&Label>,
    ) -> Put {
        let mut state = self.locked();
        let bytes = payload.bytes() as u64;
        if bytes > state.max_bytes {
            return Put::Refused;
        }
        let read = state.clock;
        state.clock += 1;
        let before = state.held.insert(
            slot,
            Slotted {
                key,
                payload: payload.clone(),
                traces: traces.to_vec(),
                label: label.cloned(),
                read,
            },
        );
        state.bytes += bytes;
        if let Some(old) = &before {
            state.bytes -= old.bytes();
        }
        state.evict(Some(slot));
        match before {
            Some(_) => Put::Replaced,
            None => Put::Slotted,
        }
    }

    pub fn held_bytes(&self) -> u64 {
        self.locked().bytes
    }

    pub fn max_bytes(&self) -> u64 {
        self.locked().max_bytes
    }

    pub fn slots(&self) -> usize {
        self.locked().held.len()
    }

    /// A lower cap evicts at once, least recently read first.
    pub fn bound(&self, max_bytes: u64) {
        let mut state = self.locked();
        state.max_bytes = max_bytes;
        state.evict(None);
    }

    pub fn clear(&self) {
        let mut state = self.locked();
        state.held.clear();
        state.bytes = 0;
    }
}
