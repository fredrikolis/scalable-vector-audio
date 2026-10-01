// Concern: a table's values, each in a slot it keeps while held, with its store place and holds | Non-concern: what a value holds (value.rs), when one goes (mod.rs) | IO: (Value) -> slot
use std::collections::{BTreeMap, HashMap};
use std::ops::{Index, IndexMut};

use super::store::Place;
use super::value::{Key, Value};

/// A value keeps its slot until let go; one made later comes later.
#[derive(Default)]
pub(crate) struct Values {
    slots: Vec<Option<Slot>>,
    free: Vec<usize>,
    order: BTreeMap<u64, usize>,
    next: u64,
    index: HashMap<Key, usize>,
}

struct Slot {
    seq: u64,
    value: Option<Value>,
    place: Place,
    held: u32,
    readers: u32,
}

impl Values {
    pub(crate) fn span(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn of(&self, key: &Key) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// `value` in a slot of its own, holding what it reads, held by nothing yet.
    pub(crate) fn push(&mut self, value: Value, place: Place) -> usize {
        let mut read = value.reads.clone();
        for at in &read {
            self.slot_mut(*at).held += 1;
        }
        read.sort_unstable();
        read.dedup();
        for at in read {
            let slot = self.slot_mut(at);
            slot.readers += 1;
            slot.place.fork = slot.readers >= 2;
        }
        let seq = self.next;
        self.next += 1;
        let key = value.key;
        let slot = Some(Slot {
            seq,
            value: Some(value),
            place,
            held: 0,
            readers: 0,
        });
        let at = match self.free.pop() {
            Some(at) => {
                self.slots[at] = slot;
                at
            }
            None => {
                self.slots.push(slot);
                self.slots.len() - 1
            }
        };
        self.order.insert(seq, at);
        self.index.insert(key, at);
        at
    }

    /// The value at `at` let go, once nothing holds it.
    pub(crate) fn remove(&mut self, at: usize) -> Value {
        let slot = self.slots[at].take().expect("a held value");
        self.order.remove(&slot.seq);
        self.free.push(at);
        let value = slot.value.expect("a value in its slot");
        if self.index.get(&value.key) == Some(&at) {
            self.index.remove(&value.key);
        }
        let mut read = value.reads.clone();
        read.sort_unstable();
        read.dedup();
        for at in read {
            let slot = self.slot_mut(at);
            slot.readers -= 1;
            slot.place.fork = slot.readers >= 2;
        }
        value
    }

    pub(crate) fn holds(&self, at: usize) -> bool {
        self.slots.get(at).is_some_and(Option::is_some)
    }

    pub(crate) fn hold(&mut self, at: usize) {
        self.slot_mut(at).held += 1;
    }

    /// `at` held once less; whether nothing holds it now.
    pub(crate) fn release(&mut self, at: usize) -> bool {
        let slot = self.slot_mut(at);
        slot.held -= 1;
        slot.held == 0
    }

    pub(crate) fn held(&self, at: usize) -> bool {
        self.slot(at).held > 0
    }

    pub(crate) fn ordered(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        self.order.values().copied()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (usize, &Value)> {
        self.ordered().map(|at| (at, &self[at]))
    }

    pub(crate) fn place_mut(&mut self, at: usize) -> &mut Place {
        &mut self.slot_mut(at).place
    }

    pub(crate) fn placed(&mut self, at: usize) -> (&mut Value, &mut Place) {
        let slot = self.slot_mut(at);
        let value = slot.value.as_mut().expect("a value in its slot");
        (value, &mut slot.place)
    }

    /// The value at `at`, out until `put` back.
    pub(crate) fn lift(&mut self, at: usize) -> Value {
        self.slot_mut(at).value.take().expect("a value in its slot")
    }

    pub(crate) fn put(&mut self, at: usize, value: Value) {
        self.slot_mut(at).value = Some(value);
    }

    fn slot(&self, at: usize) -> &Slot {
        self.slots[at].as_ref().expect("a held value")
    }

    fn slot_mut(&mut self, at: usize) -> &mut Slot {
        self.slots[at].as_mut().expect("a held value")
    }
}

impl Index<usize> for Values {
    type Output = Value;

    fn index(&self, at: usize) -> &Value {
        self.slot(at).value.as_ref().expect("a value in its slot")
    }
}

impl IndexMut<usize> for Values {
    fn index_mut(&mut self, at: usize) -> &mut Value {
        self.slot_mut(at)
            .value
            .as_mut()
            .expect("a value in its slot")
    }
}
