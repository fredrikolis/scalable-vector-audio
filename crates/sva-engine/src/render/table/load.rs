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
        for (value, need) in self.values.iter().zip(&needs) {
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

    pub(crate) fn took(&mut self, key: Hash, samples: Vec<Buffer>) {
        for value in &mut self.values {
            let Kind::Stored(stored) = &value.kind else {
                continue;
            };
            if stored.key != key {
                continue;
            }
            let lacks = value.covers();
            let lacks = lacks.minus(&value.holding());
            for part in &samples {
                for e in lacks.intersect(part.extent()).iter() {
                    value.hold(part.over(e, part.extent()));
                }
            }
        }
    }
}
