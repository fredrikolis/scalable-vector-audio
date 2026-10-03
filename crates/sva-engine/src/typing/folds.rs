// Concern: holds what the node graph folds to, per node or identity, until it or what it reads changes | Non-concern: computing a fold (refs/), when nodes change | IO: (node or identity) -> a held fold

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use sva_formula::{C64, Hash, Kept, NodeId, SpectralSum, Var};

use crate::error::EngineError;

#[derive(Debug, Default)]
pub(crate) struct Folds {
    numbers: RefCell<BTreeMap<NodeId, Option<C64>>>,
    identities: RefCell<BTreeMap<NodeId, Hash>>,
    composed: RefCell<HashMap<(Hash, Var), SpectralSum>>,
    refused: RefCell<HashMap<(NodeId, Var), EngineError>>,
    inlinable: RefCell<HashMap<(NodeId, Var), bool>>,
    written: Kept,
    #[cfg(test)]
    pub(crate) composings: std::cell::Cell<usize>,
}

/// A copy folds anew: what a node folds to is recomputed on demand, and a copy is made to be
/// changed.
impl Clone for Folds {
    fn clone(&self) -> Folds {
        Folds::default()
    }
}

impl PartialEq for Folds {
    fn eq(&self, _: &Folds) -> bool {
        true
    }
}

impl Folds {
    /// Every fold of a node in `gone`, and each sum no held identity names, let go.
    pub(crate) fn forget(&mut self, gone: &BTreeSet<NodeId>) {
        let out = |id: NodeId| gone.contains(&id);
        self.numbers.get_mut().retain(|id, _| !out(*id));
        self.identities.get_mut().retain(|id, _| !out(*id));
        self.inlinable.get_mut().retain(|(id, _), _| !out(*id));
        self.refused.get_mut().retain(|(id, _), _| !out(*id));
        self.written.forget(&out);
        let named: HashSet<Hash> = self.identities.get_mut().values().copied().collect();
        self.composed
            .get_mut()
            .retain(|(held, _), _| named.contains(held));
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

    pub(crate) fn refused(&self, key: (NodeId, Var)) -> Option<EngineError> {
        self.refused.borrow().get(&key).cloned()
    }

    pub(crate) fn keep_refused(&self, key: (NodeId, Var), refused: EngineError) {
        self.refused.borrow_mut().insert(key, refused);
    }

    pub(crate) fn inlinable(&self, key: (NodeId, Var)) -> Option<bool> {
        self.inlinable.borrow().get(&key).copied()
    }

    pub(crate) fn keep_inlinable(&self, key: (NodeId, Var), inlines: bool) {
        self.inlinable.borrow_mut().insert(key, inlines);
    }

    /// Each node's written form, read through the refs it holds.
    pub(crate) fn written(&self) -> &Kept {
        &self.written
    }
}
