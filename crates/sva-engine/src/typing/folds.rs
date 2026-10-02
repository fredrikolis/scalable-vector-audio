// Concern: holds what the node graph folds to, per node or identity, until a node changes | Non-concern: computing any fold (refs/), when nodes change | IO: (node or identity) -> a held fold or none

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use sva_formula::{C64, Hash, NodeId, SpectralSum, Var};

/// Derived from the nodes alone, so any change to a node evicts them all.
#[derive(Debug, Default)]
pub(crate) struct Folds {
    numbers: RefCell<BTreeMap<NodeId, Option<C64>>>,
    identities: RefCell<BTreeMap<NodeId, Hash>>,
    composed: RefCell<HashMap<(Hash, Var), SpectralSum>>,
    #[cfg(test)]
    pub(crate) composings: std::cell::Cell<usize>,
}

impl PartialEq for Folds {
    fn eq(&self, _: &Folds) -> bool {
        true
    }
}

impl Folds {
    pub(crate) fn clear(&mut self) {
        self.numbers.get_mut().clear();
        self.identities.get_mut().clear();
        self.composed.get_mut().clear();
    }

    pub(crate) fn number(&self, id: NodeId) -> Option<Option<C64>> {
        self.numbers.borrow().get(&id).copied()
    }

    pub(crate) fn keep_number(&self, id: NodeId, number: Option<C64>) {
        self.numbers.borrow_mut().insert(id, number);
    }

    pub(crate) fn identity(&self, id: NodeId) -> Option<Hash> {
        self.identities.borrow().get(&id).copied()
    }

    pub(crate) fn keep_identity(&self, id: NodeId, identity: Hash) {
        self.identities.borrow_mut().insert(id, identity);
    }

    pub(crate) fn composed(&self, key: (Hash, Var)) -> Option<SpectralSum> {
        self.composed.borrow().get(&key).cloned()
    }

    pub(crate) fn keep_composed(&self, key: (Hash, Var), sum: SpectralSum) {
        self.composed.borrow_mut().insert(key, sum);
    }
}
