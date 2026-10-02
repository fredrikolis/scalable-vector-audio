// Concern: which entries a store holds, their sizes and recency, as one file's text | Non-concern: the entries' bytes, when it is written (persist.rs) | IO: (text, version) <-> Index

use std::collections::HashMap;

use sva_formula::Hash;

/// Each entry's size and last use; `synced`, the clock where it last matched the file.
#[derive(Clone, Default)]
pub(super) struct Index {
    pub(super) held: HashMap<Hash, (u64, u64)>,
    clock: u64,
    synced: u64,
    pub(super) bytes: u64,
}

pub(super) fn name_of(key: Hash) -> String {
    format!("{:016x}{:016x}", key.0, key.1)
}

pub(super) fn key_of(name: &str) -> Option<Hash> {
    let hex = |s: &str| u64::from_str_radix(s, 16).ok();
    let valid = name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then(|| Some(Hash(hex(&name[..16])?, hex(&name[16..])?)))?
}

impl Index {
    /// `None` where `text` is not an index `version` wrote.
    pub(super) fn read(text: &[u8], version: &str) -> Option<Index> {
        let text = std::str::from_utf8(text).ok()?;
        let mut lines = text.lines();
        if lines.next()? != version {
            return None;
        }
        let mut index = Index::default();
        for line in lines {
            let (name, bytes) = line.split_once(' ')?;
            index.touch(key_of(name)?, bytes.parse().ok()?);
        }
        index.synced = index.clock;
        Some(index)
    }

    /// Least recently used first.
    pub(super) fn text(&self, version: &str) -> String {
        let mut out = format!("{version}\n");
        for key in self.oldest_first() {
            out += &format!("{} {}\n", name_of(key), self.held[&key].0);
        }
        out
    }

    pub(super) fn touch(&mut self, key: Hash, bytes: u64) {
        self.clock += 1;
        let at = self.clock;
        if let Some((old, _)) = self.held.insert(key, (bytes, at)) {
            self.bytes -= old;
        }
        self.bytes += bytes;
    }

    pub(super) fn used(&mut self, key: Hash) {
        self.clock += 1;
        let at = self.clock;
        if let Some((_, held)) = self.held.get_mut(&key) {
            *held = at;
        }
    }

    /// The directory's entries in `known`'s recency, one it lacks least recent.
    pub(super) fn listed(listed: Vec<(Hash, u64)>, known: &Index) -> Index {
        let at = |key| known.held.get(&key).map_or(0, |(_, at)| *at);
        let mut listed: Vec<(u64, Hash, u64)> = listed
            .into_iter()
            .map(|(k, bytes)| (at(k), k, bytes))
            .collect();
        listed.sort_unstable();
        let mut index = Index::default();
        for (_, key, bytes) in listed {
            index.touch(key, bytes);
        }
        index.synced = index.clock;
        index
    }

    pub(super) fn clock(&self) -> u64 {
        self.clock
    }

    /// The entries used after `clock`, least recent first.
    pub(super) fn used_since(&self, clock: u64) -> Vec<Hash> {
        let mut keys: Vec<(u64, Hash)> = self.held.iter().map(|(k, (_, at))| (*at, *k)).collect();
        keys.retain(|(at, _)| *at > clock);
        keys.sort_unstable();
        keys.into_iter().map(|(_, k)| k).collect()
    }

    pub(super) fn unsynced(&self) -> Vec<Hash> {
        self.used_since(self.synced)
    }

    pub(super) fn synced(&mut self) {
        self.synced = self.clock;
    }

    pub(super) fn forget(&mut self, key: Hash) {
        if let Some((bytes, _)) = self.held.remove(&key) {
            self.bytes -= bytes;
        }
    }

    pub(super) fn oldest_first(&self) -> Vec<Hash> {
        let mut keys: Vec<(u64, Hash)> = self.held.iter().map(|(k, (_, at))| (*at, *k)).collect();
        keys.sort_unstable();
        keys.into_iter().map(|(_, k)| k).collect()
    }
}
