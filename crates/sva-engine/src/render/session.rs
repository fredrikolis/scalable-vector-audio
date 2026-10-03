// Concern: what a session's renders last typed, retyping only what changed since and its readers | Non-concern: lowering a node (typing/), memory (cache/) | IO: (Instances, Order) -> Typing

use std::collections::{BTreeMap, BTreeSet};

use crate::error::EngineError;
use crate::instantiate::{Instances, Resolution};
use crate::schedule::Order;
use crate::typing::Typing;

/// Renders made in turn: each types only what changed since the last, and what reads it.
#[derive(Default)]
pub struct Session {
    pub(super) own: Typed,
    /// The composition with every volatile parameter at its stand-in.
    pub(super) stand_in: Typed,
}

#[derive(Default)]
pub(crate) struct Typed {
    held: Option<Held>,
}

struct Held {
    rate: u32,
    typed: BTreeMap<String, Resolution>,
    typing: Typing,
}

impl Typed {
    /// On a refusal it holds what it held.
    pub(crate) fn typed(
        &mut self,
        instances: &Instances,
        order: &Order,
    ) -> Result<&Typing, EngineError> {
        let rate = instances.rate();
        let held = match self.held.take() {
            Some(held) if held.rate == rate => held,
            _ => Held {
                rate,
                typed: BTreeMap::new(),
                typing: Typing::default(),
            },
        };
        let held = self.held.insert(held);
        let now: BTreeMap<String, Resolution> = instances
            .paths()
            .filter_map(|path| Some((path.to_string(), instances.resolution(path)?)))
            .collect();
        let changed = now
            .iter()
            .filter(|(path, now)| held.typed.get(*path) != Some(now))
            .map(|(path, _)| path.clone());
        let region = upward(instances, changed.collect());
        let groups: Vec<Vec<String>> = order
            .groups
            .iter()
            .filter(|group| group.iter().any(|path| region.contains(path)))
            .cloned()
            .collect();
        if let Err(refused) = held.typing.lower(instances, &groups) {
            held.typing.abort();
            return Err(refused);
        }
        let gone = held.typed.keys().filter(|path| !now.contains_key(*path));
        held.typing.hide(gone.cloned().collect::<Vec<_>>());
        held.typing.commit(instances);
        held.typed = now;
        Ok(&held.typing)
    }
}

fn upward(instances: &Instances, changed: BTreeSet<String>) -> BTreeSet<String> {
    let mut region = changed;
    let mut open: Vec<String> = region.iter().cloned().collect();
    while let Some(at) = open.pop() {
        for reader in instances.readers(&at) {
            if region.insert(reader.to_string()) {
                open.push(reader.to_string());
            }
        }
    }
    region
}
