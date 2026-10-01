// Concern: which stored samples a window asks that no stored value holds yet, and laying them in | Non-concern: reading them off a store | IO: (Table, window) -> wants; (key, samples) -> ()

use std::sync::Arc;

use sva_formula::Hash;
use sva_samples::{Buffer, Extent};

use super::Table;
use super::value::Kind;
use crate::cache::Stored;

impl Table {
    /// One stretch per stored run that `window` asks and no value holds.
    pub(crate) fn wants(&self, window: Extent) -> Vec<(Arc<Stored>, Extent)> {
        let needs = self.demand(window);
        let mut out = Vec::new();
        for (at, value) in self.values.iter() {
            let need = &needs[at];
            let Kind::Stored(stored) = &value.kind else {
                continue;
            };
            let covers = value.covers();
            let lacks = need.hold.minus(&value.holding());
            for run in covers.iter() {
                let asked = lacks.intersect(run);
                if !asked.is_empty() {
                    out.push((Arc::clone(stored), asked.hull()));
                }
            }
        }
        out
    }

    /// What `window` of `root` asks of each stored value the latest build made, and no value
    /// holds.
    pub(crate) fn wants_made(&self, root: usize, window: Extent) -> Vec<(Arc<Stored>, Extent)> {
        let needs = super::demand::demand(&self.values, &[(root, window)]);
        let mut out = Vec::new();
        for at in self.made() {
            let value = &self.values[*at];
            let Kind::Stored(stored) = &value.kind else {
                continue;
            };
            let lacks = needs[*at].hold.minus(&value.holding());
            for run in value.covers().iter() {
                let asked = lacks.intersect(run);
                if !asked.is_empty() {
                    out.push((Arc::clone(stored), asked.hull()));
                }
            }
        }
        out
    }

    pub(crate) fn took(&mut self, key: Hash, samples: &[Buffer]) {
        for at in self.values.ordered().collect::<Vec<_>>() {
            let value = &mut self.values[at];
            let Kind::Stored(stored) = &value.kind else {
                continue;
            };
            if stored.key != key {
                continue;
            }
            let lacks = value.covers();
            let lacks = lacks.minus(&value.holding());
            for part in samples {
                for e in lacks.intersect(part.extent()).iter() {
                    value.hold(part.over(e, part.extent()));
                }
            }
        }
    }
}
