// Concern: what each instance reads one hop down, and who reads it, set together | Non-concern: finding a body's reads (mod.rs), letting an instance go (build.rs) | IO: (instance, its reads) -> readers

use std::collections::{BTreeMap, BTreeSet};

/// Written only by `set` and `cleared`.
#[derive(Debug, Default)]
pub(crate) struct Edges {
    down: BTreeMap<String, Vec<String>>,
    up: BTreeMap<String, BTreeSet<String>>,
}

impl Edges {
    pub(crate) fn of(&self, node: &str) -> &[String] {
        self.down.get(node).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn readers(&self, node: &str) -> impl Iterator<Item = &str> {
        self.up.get(node).into_iter().flatten().map(String::as_str)
    }

    pub(crate) fn unread(&self, node: &str) -> bool {
        self.up.get(node).is_none_or(BTreeSet::is_empty)
    }

    pub(crate) fn set(&mut self, node: &str, deps: Vec<String>) -> Vec<String> {
        let before = self.cleared(node);
        for dep in &deps {
            self.up
                .entry(dep.clone())
                .or_default()
                .insert(node.to_string());
        }
        if !deps.is_empty() {
            self.down.insert(node.to_string(), deps);
        }
        before
    }

    pub(crate) fn cleared(&mut self, node: &str) -> Vec<String> {
        let before = self.down.remove(node).unwrap_or_default();
        for dep in &before {
            let readers = self.up.get_mut(dep).expect("a read's readers");
            readers.remove(node);
            if readers.is_empty() {
                self.up.remove(dep);
            }
        }
        before
    }
}
