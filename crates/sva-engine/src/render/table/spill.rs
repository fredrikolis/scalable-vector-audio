// Concern: hands out each staged value's samples a window held, before release drops them | Non-concern: where they go, which values are staged | IO: (Table, window) -> samples per key

use sva_formula::Hash;
use sva_samples::Buffer;

use super::segments::Segments;
use super::{Need, Table};
use crate::cache::Stored;

struct Staging {
    at: usize,
    key: Hash,
    taken: Segments,
    meta: Option<Stored>,
}

pub(crate) struct Spill {
    staged: Vec<Staging>,
    pub(crate) samples: Vec<(Hash, Buffer)>,
}

impl Spill {
    pub(crate) fn over(staged: Vec<(usize, Hash, Stored)>) -> Spill {
        Spill {
            staged: staged
                .into_iter()
                .map(|(at, key, meta)| Staging {
                    at,
                    key,
                    taken: Segments::default(),
                    meta: Some(meta),
                })
                .collect(),
            samples: Vec::new(),
        }
    }

    pub(crate) fn take(&mut self, table: &Table, asked: &[Need]) {
        for staging in &mut self.staged {
            let fresh = asked[staging.at].hold.minus(&staging.taken);
            for e in fresh.iter() {
                self.samples
                    .push((staging.key, table.samples(staging.at, e)));
            }
            staging.taken.union(&fresh);
        }
    }

    /// Each meta whose value is now handed out over its whole support.
    pub(crate) fn whole(&mut self, table: &Table) -> Vec<(Hash, Stored)> {
        let mut out = Vec::new();
        for staging in &mut self.staged {
            let support = table.values[staging.at].support;
            if support.is_bounded() && staging.taken.covers(&Segments::of(support)) {
                out.extend(handed(staging, table));
            }
        }
        out
    }

    pub(crate) fn rest(&mut self, table: &Table) -> Vec<(Hash, Stored)> {
        self.staged
            .iter_mut()
            .filter_map(|staging| handed(staging, table))
            .collect()
    }
}

/// A label settles once the value is computed, so it is read as the meta leaves.
fn handed(staging: &mut Staging, table: &Table) -> Option<(Hash, Stored)> {
    let mut meta = staging.meta.take()?;
    meta.label = table.label(staging.at);
    Some((staging.key, meta))
}
