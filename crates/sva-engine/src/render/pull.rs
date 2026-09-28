// Concern: a whole render as a stream pulled to its end, what it holds whole first and where `until` ended it | Non-concern: one node's samples (mod.rs, drive/) | IO: (&mut Render) -> every buffer

use std::collections::{BTreeMap, BTreeSet};

use sva_formula::{Hash, Held, NodeId};
use sva_samples::{Buffer, Extent, Label, NodeRenderer};

use super::drive::node::{self, Hold, Kind};
use super::drive::{self, Driver};
use super::{
    Lenses, Render, affordable, answer, extent, materialize, reach, reads, sampled, store,
    unlooped, warm,
};
use crate::cache::{Expected, Payload};
use crate::cast::Cast;
use crate::error::EngineError;
use crate::typing::Value;

/// Any size writes the same bits.
const BLOCK: usize = 1 << 12;

/// What a render needs before any reader runs is held whole first, and the stream's own
/// driver pulls every other node. A render `until` stopped before is pulled to that stop, and
/// what it pulled then answers from the store.
pub(super) fn computed(
    held: &mut Render,
    lenses: &Lenses,
    looped: &BTreeMap<String, String>,
    costed: &[NodeId],
) -> Result<(), EngineError> {
    let stop_key = stop_key(held, lenses)?;
    let recalled = stop_key.and_then(|key| recalled(held, lenses, key));
    affordable(held)?;
    let needed = lenses.needed(held);
    unlooped(held, &needed, looped)?;
    let keys = keys(held, lenses, &needed, costed, recalled)?;
    let (whole, stored) = whole(held, lenses, &needed, &keys);
    for id in held.schedule.materialize.clone() {
        let run = stored.contains(&id) && lenses.runs.contains(&id);
        if !needed.contains(&id) || !whole.contains(&id) || run {
            continue;
        }
        match (stored.contains(&id), keys.get(&id)) {
            (true, Some(&(key, samples))) => {
                let lens = lenses.at(id);
                let (hit, label) = warm(held, id, key, samples, lens.as_ref())
                    .expect("a stored node answers from the store");
                held.buffers.insert(id, hit);
                held.labels.insert(id, label);
            }
            _ => materialize(held, id, lenses)?,
        }
    }
    pulled(held, lenses, &needed, costed, &keys, (stop_key, recalled))
}

/// Each sampled node's key over what it is pulled to: its extent, or where a recalled stop
/// cuts it, as a render ending there would cut it.
fn keys(
    held: &Render,
    lenses: &Lenses,
    needed: &BTreeSet<NodeId>,
    costed: &[NodeId],
    recalled: Option<i64>,
) -> Result<BTreeMap<NodeId, (Hash, usize)>, EngineError> {
    let cut = match (recalled, held.range) {
        (Some(stop), Some(range)) => Some(reach::decided(
            held,
            costed,
            Extent::new(range.start, stop),
        )?),
        _ => None,
    };
    let mut out = BTreeMap::new();
    for &id in &held.schedule.materialize {
        let pulled = needed.contains(&id) && matches!(held.tys.ty(id).held, Held::Sampled);
        if !pulled || lenses.at(id).is_none() || lenses.runs.contains(&id) {
            continue;
        }
        let extent = cut.as_ref().map_or(held.extents.of(id), |cut| cut.of(id));
        if !extent.is_empty() {
            out.insert(id, sampled::key_over(held, id, extent)?);
        }
    }
    Ok(out)
}

/// A closed form's row is fitted and keyed to its whole extent, a transform reads its input
/// whole, a program reading ahead reads its input there, and a stored node answers whole from
/// the store; what any of them reads is whole before it. The second set is what the store
/// answers.
fn whole(
    held: &Render,
    lenses: &Lenses,
    needed: &BTreeSet<NodeId>,
    keys: &BTreeMap<NodeId, (Hash, usize)>,
) -> (BTreeSet<NodeId>, BTreeSet<NodeId>) {
    let (mut out, mut stored) = (BTreeSet::new(), BTreeSet::new());
    let holds = |id: NodeId| {
        let recording = lenses.recording?;
        keys.get(&id).map(|(key, _)| recording.holds(*key))
    };
    for &id in held.schedule.materialize.iter().rev() {
        if !needed.contains(&id) {
            continue;
        }
        let forced = out.contains(&id)
            || !matches!(held.tys.ty(id).held, Held::Sampled)
            || matches!(held.tys.value(id), Value::Cast(Cast::Istft, _))
            || reads_ahead(held, id);
        let answered = !forced
            && match lenses.runs.contains(&id) {
                true => lenses.answered(held, id),
                false => holds(id) == Some(true),
            };
        if answered {
            stored.insert(id);
        }
        if forced || answered {
            out.insert(id);
        }
        if forced {
            out.extend(reads(held, id));
        }
    }
    (out, stored)
}

fn reads_ahead(held: &Render, id: NodeId) -> bool {
    let Ok(program) = sampled::program(held, id) else {
        return true;
    };
    let mut ahead = false;
    extent::leaves(&program.renderer, &mut |leaf| {
        if let NodeRenderer::Buffer { at, .. } = leaf {
            ahead |= at.ahead();
        }
    });
    ahead
}

/// Pulls every node not yet whole to the range's end, or to where `until` holds, which ends
/// the range there. A root no reading materializes is read on the grid for `until` alone, and
/// not where the stop is recalled.
fn pulled(
    held: &mut Render,
    lenses: &Lenses,
    needed: &BTreeSet<NodeId>,
    costed: &[NodeId],
    keys: &BTreeMap<NodeId, (Hash, usize)>,
    (stop_key, recalled): (Option<Hash>, Option<i64>),
) -> Result<(), EngineError> {
    let Some(range) = held.range else {
        return Ok(());
    };
    let order: Vec<NodeId> = held
        .schedule
        .materialize
        .iter()
        .copied()
        .filter(|id| needed.contains(id) && !matches!(held.tys.ty(*id).held, Held::Frames))
        .collect();
    let (pulled, until) = match recalled {
        Some(stop) => (Extent::new(range.start, stop), None),
        None => (range, held.config.until.clone()),
    };
    let aside = !order.contains(&held.root) && until.is_some();
    for &id in order.iter().filter(|id| !held.buffers.contains_key(id)) {
        if let Some(&(key, samples)) = keys.get(&id) {
            warm(held, id, key, samples, lenses.at(id).as_ref());
        }
    }
    let whole: BTreeMap<NodeId, Buffer> = order
        .iter()
        .filter_map(|id| Some((*id, held.buffers.remove(id)?)))
        .collect();
    let mut nodes = node::built(held, &order, Hold::Every(whole), lenses, &|_| false)?;
    if aside {
        let root = answer::on_the_grid(held, held.root)?;
        let support = extent::Supports::new(&held.tys, held.config.rate).of(held.root);
        nodes.push(node::whole(held.root, root, range, support));
    }
    let root = nodes.iter().position(|n| n.id == held.root);
    let mut driver = Driver::new(nodes, root, pulled, BLOCK, until, &held.config, false);
    while driver.pull(held, lenses)? {}
    let stop = recalled.or(driver.stop().filter(|stop| *stop < range.end));
    let mut driven = Vec::new();
    for mut node in driver.nodes {
        if aside && node.id == held.root {
            continue;
        }
        node.store(held, lenses);
        let computed = !matches!(node.kind, Kind::Whole);
        if computed || node.run.is_some() {
            let label = Label::measured(held.config.profile.name, held.config.rate);
            held.labels.insert(node.id, label);
        }
        if computed {
            driven.push(node.id);
        }
        let buffer = node.tape.into_buffer(held.config.rate);
        held.buffers.insert(node.id, buffer);
    }
    if let Some(stop) = stop {
        reach::extend(held, costed, Extent::new(range.start, stop))?;
        if let (Some(key), None) = (stop_key, recalled) {
            let record = Buffer::mono(held.config.rate, vec![range.end as f64, stop as f64]);
            record_lens(held, lenses).store(key, &Payload::Samples(Box::new(record)), None);
        }
    }
    for id in driven {
        stored(held, lenses, id, keys.get(&id).map(|(key, _)| *key))?;
    }
    Ok(())
}

/// Every driven node is stored over its extent, which a stop cut short of what it was looked
/// up under: every sample before the cut is the same.
fn stored(
    held: &mut Render,
    lenses: &Lenses,
    id: NodeId,
    looked: Option<Hash>,
) -> Result<(), EngineError> {
    let (extent, lens) = (held.extents.of(id), lenses.at(id));
    let (Some(lens), Some(looked)) = (lens, looked) else {
        return Ok(());
    };
    let buffer = &held.buffers[&id];
    if extent.is_empty() || buffer.extent().end < extent.end {
        return Ok(());
    }
    let cut = buffer.over(extent, held.extents.support(id));
    let (key, _) = sampled::key(held, id)?;
    lens.rekey(looked, key);
    store(key, &cut, &held.labels[&id], Some(&lens));
    held.buffers.insert(id, cut);
    Ok(())
}

const STOP_TAG: u64 = 0x73_74_6f_70_00_00_00_02;

/// One root over one range, stopped by one condition read at one frame.
fn stop_key(held: &Render, lenses: &Lenses) -> Result<Option<Hash>, EngineError> {
    let (Some(until), Some(range), Some(_)) = (&held.config.until, held.range, lenses.recording)
    else {
        return Ok(None);
    };
    let config = &held.config;
    let mut words = vec![
        u64::from(config.rate),
        range.start as u64,
        range.end as u64,
        config.profile.precision_bits as u64,
        drive::frame(config) as u64,
        STOP_TAG,
    ];
    words.extend(until.to_string().bytes().map(u64::from));
    Ok(Some(crate::cache::mixed(held.identity(held.root)?, &words)))
}

/// A volatile root keeps one stop, its last, beside its one value.
fn record_lens<'l>(held: &Render, lenses: &'l Lenses) -> crate::cache::Lens<'l> {
    let recording = lenses
        .recording
        .expect("a stop is kept only where a store is");
    let slot = lenses.slot(held.root);
    recording.at(
        slot.map(|s| crate::cache::mixed(s, &[STOP_TAG])),
        false,
        true,
    )
}

fn recalled(held: &Render, lenses: &Lenses, key: Hash) -> Option<i64> {
    let range = held.range?;
    let expected = Expected::Samples {
        rate: held.config.rate,
        width: 1,
        samples: 2,
    };
    let lens = record_lens(held, lenses);
    let entry = lens.load(key, held.tys.name(held.root), expected)?;
    let record = entry.payload.samples()?.plane(0).to_vec();
    (record[0] as i64 == range.end).then_some(record[1] as i64)
}
