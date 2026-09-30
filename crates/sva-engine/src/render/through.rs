// Concern: renders from the root down through a persistent store, staging what it computes | Non-concern: the store's medium and commit, a render with no store | IO: (&Graph, target, Store) -> Render

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;
use sva_formula::{Hash, Held as Representation, NodeId};

use super::table::spill::Spill;
use super::table::{self, Table};
use super::{Render, RenderConfig, closed, drive, driving, drove, frontier, planned_over};
use crate::cache::{Backend, CacheStats, Outcome, Recording, Store, Stored, Through};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate;
use crate::schedule;
use crate::typing::{self, Typing};

/// `render` through `store`, from the root down: a node the store answers stands as its
/// samples, and nothing under it is typed, planned or looked up. What the rest computes is
/// staged beside the store as the render drops it; only `persist` writes the store.
pub async fn render_through<B: Backend>(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    store: &Store<B>,
) -> Result<Render, EngineError> {
    let instances = instantiate::instantiate(graph, target, config.rate)?;
    let root = instances.instance_of(target)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&root))?;
    let keys = keys(graph, &instances, &order, &config);
    let mut found = frontier::Frontier::from((&instances, &order), &keys, &root, (&config, false));
    let (mut held, typed) = loop {
        found.walk(store).await;
        let tys = typing::infer_over(&instances, &order.within(&found.visited), &found.stored)?;
        let id = tys
            .id(&root)
            .ok_or_else(|| EngineError::UnknownNode(root.clone()))?;
        let bounds: BTreeSet<NodeId> = keys
            .keys()
            .filter_map(|path| tys.id(path))
            .filter(|id| readable(&tys, *id))
            .collect();
        let typed = tys.lowered().to_vec();
        let mut held = planned_over(&instances, (tys, id), config.clone(), &bounds)?;
        let short = match (&mut held.table, held.range) {
            (Some(table), Some(range)) => {
                load(table, store, range).await;
                short(table, range)
            }
            _ => Vec::new(),
        };
        if short.is_empty() {
            break (held, typed);
        }
        for path in short {
            found.reopen(&path);
        }
    };
    let staging = staging(&held, &keys, &found);
    let mut kept = BTreeSet::new();
    if let Some(mut driver) = driving(&mut held, Recording::over(None, None))? {
        driver.spill = Some(Spill::over(staging));
        while spilled(&mut driver, store, &mut kept).await? {}
        let rest = driver.spill.take().map(|mut s| s.rest(&driver.table));
        for (key, meta) in rest.into_iter().flatten() {
            store.stage_meta(key, &meta).await.map_err(unstaged)?;
            kept.insert(key);
        }
        drove(&mut held, driver);
    }
    let computed = held.table.as_ref().map_or(Vec::new(), |table| {
        let computed = table.values.iter();
        let computed = computed.filter(|v| !matches!(v.kind, table::Kind::Stored { .. }));
        computed.map(|v| v.name.clone()).collect()
    });
    let mut lookups = found.lookups;
    for lookup in &mut lookups {
        if lookup.outcome == Outcome::ComputedNotStored && kept.contains(&lookup.key) {
            lookup.outcome = Outcome::ComputedStored;
        }
    }
    let reached = held.cache_stats.take().map_or(Vec::new(), |stats| {
        let walked = lookups.len();
        stats.reached.iter().map(|(at, _)| (*at, walked)).collect()
    });
    held.cache_stats = Some(CacheStats {
        lookups,
        reached,
        typed,
        planned: computed,
        ..CacheStats::default()
    });
    closed(&mut held)?;
    Ok(held)
}

/// Each instance's store key: its source identity at the render's rate and profile.
pub(crate) fn keys(
    graph: &Graph,
    instances: &instantiate::Instances,
    order: &schedule::Order,
    config: &RenderConfig,
) -> BTreeMap<String, Hash> {
    crate::source::identities(graph, instances, order)
        .into_iter()
        .map(|(path, identity)| {
            let key = crate::cache::node_key(identity, config.rate, &config.profile);
            (path, key)
        })
        .collect()
}

/// What `window` asks of each stored value's samples, read off `store`; what it cannot read
/// stays short.
pub(crate) async fn load(table: &mut Table, store: &impl Through, window: sva_samples::Extent) {
    for (stored, over) in table.wants(window) {
        if let Some(samples) = store.read(&stored, over).await {
            table.took(stored.key, samples);
        }
    }
}

/// Each stored node whose samples miss some its readers ask over `range`.
fn short(table: &Table, range: sva_samples::Extent) -> Vec<String> {
    let needs = table.demand(range);
    table
        .values
        .iter()
        .zip(&needs)
        .filter(|(value, need)| {
            matches!(value.kind, table::Kind::Stored { .. }) && !need.compute.is_empty()
        })
        .map(|(value, _)| value.name.clone())
        .collect()
}

/// One block pulled, and what it handed out moved into the staging area.
async fn spilled<B: Backend>(
    driver: &mut drive::Driver,
    store: &Store<B>,
    kept: &mut BTreeSet<Hash>,
) -> Result<bool, EngineError> {
    let more = driver.pull()?;
    let spill = driver
        .spill
        .as_mut()
        .expect("a render through a store spills");
    for (key, samples) in std::mem::take(&mut spill.samples) {
        store.stage(key, &samples).await.map_err(unstaged)?;
    }
    for (key, meta) in spill.whole(&driver.table) {
        store.stage_meta(key, &meta).await.map_err(unstaged)?;
        kept.insert(key);
    }
    Ok(more)
}

fn unstaged(why: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "store.unwritable".to_string(),
        message: format!("what the render computed could not be staged beside the store: {why}"),
        location: Located::default(),
        help: "free the store's disk, move it with `--cache <path>`, or pass `--cache none`"
            .to_string(),
    })
}

/// A node another may read as its samples alone: samples, and nothing that reads it between
/// them.
fn readable(tys: &Typing, id: NodeId) -> bool {
    tys.ty(id).held == Representation::Sampled && !schedule::anywhere(tys, id)
}

/// Each value a missed node computes, at its own node's key: the root whatever it is, any
/// other where a reader may take its samples. One that only moves a value stored or staged
/// stands for that value's samples, holding none of its own.
fn staging(
    held: &Render,
    keys: &BTreeMap<String, Hash>,
    found: &frontier::Frontier<'_>,
) -> Vec<(usize, Hash, Stored)> {
    let Some(table) = &held.table else {
        return Vec::new();
    };
    let tys = &held.tys;
    let mut out = Vec::new();
    for (path, key) in keys {
        if found.stored.contains_key(path) || !found.visited.contains(path) {
            continue;
        }
        let Some((id, at)) = tys.id(path).and_then(|id| Some((id, table.of(id)?))) else {
            continue;
        };
        let value = &table.values[at];
        let own = tys.name(id) == path;
        let readable = readable(tys, id) && value.alias().is_none();
        let kept = value.pure
            && value.period.is_none()
            && !matches!(value.kind, table::Kind::Frames { .. });
        if !own || !kept || !(at == table.root || readable) {
            continue;
        }
        let (priced, moved) = under(table, at);
        let ty = tys.ty(id);
        let meta = Stored {
            key: *key,
            samples: Default::default(),
            label: table.label(at),
            width: u8::try_from(value.width).expect("a width the typing held"),
            codomain: ty.codomain,
            rate: ty.rate,
            grid: tys.grid(id),
            support: value.support,
            priced,
            moved,
            readable,
        };
        out.push((at, *key, meta));
    }
    let staged: BTreeMap<usize, Hash> = out.iter().map(|(at, key, _)| (*at, *key)).collect();
    for (at, _, meta) in &mut out {
        let Some((moved, by)) = moves(table, *at) else {
            continue;
        };
        let stored = table.values[moved].node.and_then(|id| {
            let (file, shift) = found.stored.get(tys.name(id))?.file()?;
            Some((file, by - shift))
        });
        let staged = staged.get(&moved).map(|key| (*key, by));
        if let Some((of, by)) = staged.or(stored) {
            *meta = meta.clone().referring(of, by);
        }
    }
    out
}

/// The value `at` only moves, through every move between, and by how much.
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
