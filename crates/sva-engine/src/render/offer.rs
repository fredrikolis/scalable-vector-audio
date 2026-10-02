// Concern: which values a render offers memory as nodes, with their meta, and when | Non-concern: whether memory keeps or writes them | IO: (Render, keys, frontier) -> offers; (Table, Memory) -> offered

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_formula::{Hash, Held as Representation, NodeId};
use sva_samples::Extent;

use super::Render;
use super::frontier::Frontier;
use super::table::segments::Segments;
use super::table::{self, Table};
use crate::cache::{Memory, Offered, Stored};
use crate::schedule;
use crate::typing::Typing;

struct Offer {
    at: usize,
    stored: Stored,
    own: Own,
    slot: Option<Hash>,
}

enum Own {
    /// Another offered or resident node's, moved.
    Moves,
    Shares(Offered),
    /// Nowhere a value keeps them: a period's samples, laid out over the range.
    Copies,
}

/// The values a render offers, each once its whole support is computed or the render ends.
pub(crate) struct Offers {
    pending: Vec<Offer>,
    keys: BTreeMap<usize, Hash>,
    range: Extent,
}

impl Offers {
    /// Each value a missed node computes, at its own node's key: the root whatever it is, any
    /// other where a reader may take its samples.
    pub(crate) fn of(
        held: &mut Render,
        (keys, found): (&BTreeMap<String, Hash>, &Frontier),
        memory: &Memory,
    ) -> Offers {
        let mut pending = Vec::new();
        if let Some(table) = &mut held.table {
            let tys = &held.tys;
            for (path, key) in keys {
                if found.stored.contains_key(path) || !found.visited.contains(path) {
                    continue;
                }
                let Some((id, at)) = tys.id(path).and_then(|id| Some((id, table.of(id)?))) else {
                    continue;
                };
                if let Some(mut stored) = offerable(table, tys, (id, at), (path, *key)) {
                    let beneath = found.beneath(path);
                    stored.cuts = table.cuts_of(tys, |name| beneath.contains(name));
                    pending.push(Offer {
                        at,
                        stored,
                        own: Own::Moves,
                        slot: table.slot(at),
                    });
                }
            }
        }
        let keys: BTreeMap<usize, Hash> = pending.iter().map(|o| (o.at, o.stored.key)).collect();
        if let Some(table) = &mut held.table {
            for offer in &mut pending {
                if moved(table, offer.at, &keys).is_none() {
                    offer.own = table.offered(offer.at).map_or(Own::Copies, Own::Shares);
                }
                if let Own::Shares(source) = &offer.own {
                    let stored = offer.stored.clone();
                    memory.offer(stored, source.clone(), (offer.slot, false));
                }
            }
        }
        Offers {
            pending,
            keys,
            range: held.range.unwrap_or(Extent::NOWHERE),
        }
    }

    /// Each value whose whole support is computed, offered; one that moves another waits for
    /// the end, so what it moves is offered first.
    pub(crate) fn whole(&mut self, table: &Table, memory: &Memory) {
        let done = |offer: &Offer| {
            let value = &table.values[offer.at];
            if !matches!(offer.own, Own::Shares(_)) {
                return false;
            }
            let support = value.support();
            let mut computed = Segments::default();
            value.evaluated.iter().for_each(|e| computed.add(*e));
            support.is_bounded() && computed.covers(&Segments::of(support))
        };
        let (whole, rest): (Vec<Offer>, Vec<Offer>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(done);
        self.pending = rest;
        self.offer(whole, table, memory);
    }

    /// Every value not yet offered, each that moves another after the rest.
    pub(crate) fn rest(&mut self, table: &Table, memory: &Memory) {
        let mut rest = std::mem::take(&mut self.pending);
        rest.sort_by_key(|offer| matches!(offer.own, Own::Moves));
        self.offer(rest, table, memory);
    }

    fn offer(&mut self, offers: Vec<Offer>, table: &Table, memory: &Memory) {
        for mut offer in offers {
            offer.stored.label = table.label(offer.at);
            let source = match offer.own {
                Own::Shares(source) => source,
                Own::Moves => match moved(table, offer.at, &self.keys) {
                    Some(source) => source,
                    None => continue,
                },
                Own::Copies => {
                    let over = self.range.intersect(offer.stored.support);
                    Offered::Held(vec![Arc::new(table.samples(offer.at, over))])
                }
            };
            memory.offer(offer.stored, source, (offer.slot, true));
        }
    }
}

fn offerable(
    table: &Table,
    tys: &Typing,
    (id, at): (NodeId, usize),
    (path, key): (&str, Hash),
) -> Option<Stored> {
    let value = &table.values[at];
    let own = tys.name(id) == path;
    let readable = readable(tys, id) && value.alias().is_none();
    let kept =
        value.pure && value.period.is_none() && !matches!(value.kind, table::Kind::Frames { .. });
    if !own || !kept || !(at == table.root || readable) {
        return None;
    }
    let (priced, moved) = under(table, at);
    let ty = tys.ty(id);
    Some(Stored {
        key,
        label: table.label(at),
        width: u8::try_from(value.width).expect("a width the typing held"),
        codomain: ty.codomain,
        rate: ty.rate,
        grid: tys.grid(id),
        support: value.support(),
        priced,
        moved,
        readable,
        sampled: ty.held == Representation::Sampled,
        cuts: Vec::new(),
        held: Vec::new(),
    })
}

/// A node another may read as its samples alone: samples, and nothing that reads it between
/// them.
pub(crate) fn readable(tys: &Typing, id: NodeId) -> bool {
    tys.ty(id).held == Representation::Sampled && !schedule::anywhere(tys, id)
}

fn moved(table: &Table, at: usize, offered: &BTreeMap<usize, Hash>) -> Option<Offered> {
    let (moved, by) = moves(table, at)?;
    let resident = match &table.values[moved].kind {
        table::Kind::Resident(stored) => Some(stored.key),
        _ => None,
    };
    let of = offered.get(&moved).copied().or(resident)?;
    Some(Offered::Moves { of, by })
}

fn moves(table: &Table, at: usize) -> Option<(usize, i64)> {
    let (mut read, mut by) = table.values[at].moves()?;
    while let Some((next, shift)) = table.values[read].moves() {
        (read, by) = (next, by + shift);
    }
    Some((read, by))
}

/// What a value and every value under it cost over the range, and the most any moved a read.
fn under(table: &Table, at: usize) -> (u128, f64) {
    let (mut seen, mut open) = (BTreeSet::from([at]), vec![at]);
    let (mut priced, mut moved) = (0u128, 0.0f64);
    while let Some(at) = open.pop() {
        priced += table.planned[at];
        moved = moved.max(table.values[at].moved);
        open.extend(
            table.values[at]
                .reads
                .iter()
                .filter(|read| seen.insert(**read)),
        );
    }
    (priced, moved)
}
