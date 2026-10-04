// Concern: what a typing lowers beside the one it holds, until committed or let go | Non-concern: lowering a node (lower/), which paths changed | IO: (paths hidden, nodes made) -> ids freed

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_formula::NodeId;

use super::{SumSlot, Typing};
use crate::arguments::Arguments;
use crate::instantiate::Instances;
use crate::time::Grid;

/// What a draft did, in order, so a pass of a loop can be undone back to where it began.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Node(NodeId, (Arc<str>, Grid)),
    Origin(u32, (Arc<str>, Grid)),
    Path(String, Option<NodeId>),
    Copy((String, Grid), Option<NodeId>),
    Pending(NodeId),
    Noted(String, Option<Arguments>),
}

/// What a typing lowers beside the nodes it holds: the paths it lowers anew, hidden from what
/// is held, and every name it gives until committed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Draft {
    pub(super) hidden: BTreeSet<String>,
    pub(super) by_path: BTreeMap<String, NodeId>,
    pub(super) copies: BTreeMap<(String, Grid), NodeId>,
    pub(super) arguments: BTreeMap<String, Arguments>,
    pub(super) sum: Option<Option<(NodeId, Vec<SumSlot>)>>,
    pub(super) journal: Vec<Entry>,
}

/// What lowering one path wrote, on each grid it was lowered on, and the file it is of.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Units {
    file: String,
    grids: Vec<(Grid, Unit)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Unit {
    nodes: Vec<NodeId>,
    origins: Vec<u32>,
}

/// Where a draft stood, for a pass to go back to.
pub(crate) struct Mark {
    journal: usize,
    lowered: usize,
}

impl Typing {
    /// Hides each of `paths` from what is held: each is lowered anew, or let go on commit.
    pub(crate) fn hide(&mut self, paths: impl IntoIterator<Item = String>) {
        self.draft.hidden.extend(paths);
    }

    pub(crate) fn checkpoint(&self) -> Mark {
        Mark {
            journal: self.draft.journal.len(),
            lowered: self.lowered.len(),
        }
    }

    /// Undoes what the draft did since `mark`.
    pub(crate) fn rollback(&mut self, mark: Mark) {
        let mut undone = Vec::new();
        while self.draft.journal.len() > mark.journal {
            match self.draft.journal.pop().expect("an entry past the mark") {
                Entry::Node(id, _) => {
                    self.place(id, None);
                    self.free.push(id.0);
                    self.pending.remove(&id);
                    undone.push(id);
                }
                Entry::Origin(token, _) => self.free_origin(token),
                Entry::Path(path, old) => restore(&mut self.draft.by_path, path, old),
                Entry::Copy(key, old) => restore(&mut self.draft.copies, key, old),
                Entry::Pending(id) => {
                    self.pending.remove(&id);
                }
                Entry::Noted(path, old) => restore(&mut self.draft.arguments, path, old),
            }
        }
        self.lowered.truncate(mark.lowered);
        self.forget(undone);
    }

    /// Lets the draft go: every node it made is freed, and those ids answered.
    pub(crate) fn abort(&mut self) -> Vec<NodeId> {
        let made = self.draft.journal.iter().filter_map(|entry| match entry {
            Entry::Node(id, _) => Some(*id),
            _ => None,
        });
        let freed = made.collect();
        self.rollback(Mark {
            journal: 0,
            lowered: 0,
        });
        let renamed = self.draft.sum.take().flatten().map(|(sum, _)| sum);
        self.draft = Draft::default();
        self.forget(renamed);
        freed
    }

    /// The draft held in place of what it hid: each hidden path's own nodes are freed, and
    /// those ids answered; each file whose instances it touched is named again.
    pub(crate) fn commit(&mut self, inst: &Instances) -> Vec<NodeId> {
        let draft = std::mem::take(&mut self.draft);
        let mut touched = BTreeSet::new();
        let mut freed = Vec::new();
        for path in &draft.hidden {
            if let Some(units) = self.units.remove(path) {
                for (_, unit) in units.grids {
                    for id in unit.nodes {
                        self.place(id, None);
                        self.free.push(id.0);
                        freed.push(id);
                    }
                    for token in unit.origins {
                        self.free_origin(token);
                    }
                }
                touched.insert(units.file);
            }
            self.by_path.remove(path);
            self.copies.remove(path);
            self.arguments.remove(path);
        }
        for entry in draft.journal {
            let (unit, (path, grid)) = match entry {
                Entry::Node(id, at) => ((Some(id), None), at),
                Entry::Origin(token, at) => ((None, Some(token)), at),
                _ => continue,
            };
            let units = self.units.entry(path.to_string()).or_insert_with(|| Units {
                file: inst.origin(&path).unwrap_or(&path).to_string(),
                grids: Vec::new(),
            });
            touched.insert(units.file.clone());
            let held = match units.grids.iter().position(|(g, _)| *g == grid) {
                Some(at) => &mut units.grids[at].1,
                None => {
                    units.grids.push((grid, Unit::default()));
                    &mut units.grids.last_mut().expect("just pushed").1
                }
            };
            match unit {
                (Some(id), _) => held.nodes.push(id),
                (_, Some(token)) => held.origins.push(token),
                _ => {}
            }
        }
        self.by_path.extend(draft.by_path);
        for ((path, grid), id) in draft.copies {
            self.copies.entry(path).or_default().insert(grid, id);
        }
        self.arguments.extend(draft.arguments);
        if let Some(sum) = draft.sum {
            self.sum = sum;
        }
        for file in touched {
            self.name_file(inst, &file);
        }
        self.forget(freed.iter().copied());
        freed
    }

    /// A file with one instance answers to its own name too.
    fn name_file(&mut self, inst: &Instances, file: &str) {
        if self.aliases.remove(file) {
            self.by_path.remove(file);
        }
        let held: Vec<NodeId> = inst
            .instances_of(file)
            .filter_map(|p| self.by_path.get(&p).copied())
            .collect();
        if let ([only], None) = (held.as_slice(), self.by_path.get(file)) {
            self.by_path.insert(file.to_string(), *only);
            self.aliases.insert(file.to_string());
        }
        match held.is_empty() {
            true => self.files.remove(file),
            false => self.files.insert(file.to_string(), held),
        };
    }

    fn free_origin(&mut self, token: u32) {
        let held = self.origins[token as usize].take();
        let (site, _) = held.expect("an origin marked once");
        self.free_origins.push(token);
        let held = self.sites[site as usize]
            .as_mut()
            .expect("a site an origin marks");
        held.1 -= 1;
        if held.1 == 0 {
            let (name, _) = self.sites[site as usize].take().expect("a site");
            self.site_ids.remove(&name);
            self.free_sites.push(site);
        }
    }
}

fn restore<K: Ord, V>(map: &mut BTreeMap<K, V>, key: K, old: Option<V>) {
    match old {
        Some(old) => {
            map.insert(key, old);
        }
        None => {
            map.remove(&key);
        }
    }
}
