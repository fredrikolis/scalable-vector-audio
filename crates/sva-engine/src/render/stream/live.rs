// Concern: the call-site states and recent samples a stream holds at a block's end, read for a silence proof | Non-concern: the bound itself (silent/envelope.rs) | IO: (&[Streamed], position) -> Live

use std::collections::BTreeMap;

use sva_formula::NodeId;
use sva_samples::{FilterSite, Solver};

use super::node::{Kind, Streamed};
use crate::render::silent::Live;

pub(super) struct View<'a> {
    nodes: &'a [Streamed],
    index: BTreeMap<NodeId, usize>,
    /// Each call site's node, its stream node and its site within that machine.
    sites: BTreeMap<NodeId, (usize, usize)>,
    at: usize,
}

impl<'a> View<'a> {
    pub(super) fn of(nodes: &'a [Streamed], at: usize) -> View<'a> {
        let mut sites = BTreeMap::new();
        for (i, node) in nodes.iter().enumerate() {
            if let Kind::Machine { sites: own, .. } = &node.kind {
                for (site, id) in own.iter().enumerate() {
                    sites.entry(*id).or_insert((i, site));
                }
            }
        }
        View {
            index: nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect(),
            nodes,
            sites,
            at,
        }
    }

    fn machine(&self, id: NodeId) -> Option<(&sva_samples::Machine, usize)> {
        let &(node, site) = self.sites.get(&id)?;
        match &self.nodes[node].kind {
            Kind::Machine { machine, .. } => Some((machine, site)),
            _ => None,
        }
    }
}

impl Live for View<'_> {
    fn solver(&self, id: NodeId) -> Option<&dyn Solver> {
        let (machine, site) = self.machine(id)?;
        machine.solver(site)
    }

    fn filter(&self, id: NodeId) -> Option<&FilterSite> {
        let (machine, site) = self.machine(id)?;
        machine.filter(site)
    }

    /// Samples before the grid's first are zero; one the tape has let go is not known.
    fn history(&self, id: NodeId, back: i64) -> Option<f64> {
        let tape = &self.nodes[*self.index.get(&id)?].tape;
        let from = (self.at as i64 + back).max(0) as usize;
        if from >= self.at {
            return Some(0.0);
        }
        (from >= tape.base() && tape.end() >= self.at).then_some(())?;
        let widest = (0..tape.width())
            .flat_map(|c| tape.since(c, from)[..self.at - from].iter())
            .fold(0.0f64, |held, v| held.max(v.abs()));
        Some(widest)
    }
}
