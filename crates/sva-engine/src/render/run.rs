// Concern: renders or warms a target over memory, from the root down to what it answers | Non-concern: what memory keeps or writes, computing a value | IO: (&Graph, target, Tier) -> Render, CacheStats

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;
use sva_formula::{Hash, NodeId};

use super::offer::{Offers, readable};
use super::table::{self, Table};
use super::{Render, RenderConfig, closed, driving, drove, frontier, planned_over};
use crate::cache::{Backend, CacheStats, Recording, Tier};
use crate::error::EngineError;
use crate::instantiate;
use crate::schedule;
use crate::typing;

/// `target` over `tier`, from the root down: a node memory answers stands as its samples, and
/// nothing under it is typed, planned or looked up. What the rest computes memory keeps as it
/// says, offered as nodes where a reader may take them; only `persist` commits them to a disk.
/// A failing disk fails no render: memory writes nothing more to it, and the stats say why.
pub async fn render_over<B: Backend>(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
) -> Result<Render, EngineError> {
    match run(graph, target, config, tier, Keep::Wanted).await? {
        Reached::Render(held) => Ok(*held),
        Reached::Held(_) => unreachable!("a render reads a held root's samples"),
    }
}

/// `render_over`'s work alone, `config` asking nothing: a root memory answers ends it unread.
pub async fn warm<B: Backend>(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
) -> Result<CacheStats, EngineError> {
    Ok(
        match run(graph, target, config, tier, Keep::Nothing).await? {
            Reached::Render(mut held) => held.cache_stats.take().expect("a render reports"),
            Reached::Held(stats) => *stats,
        },
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    Wanted,
    Nothing,
}

enum Reached {
    Render(Box<Render>),
    Held(Box<CacheStats>),
}

async fn run<B: Backend>(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
    keep: Keep,
) -> Result<Reached, EngineError> {
    let round = tier.begin();
    let mut recording = Recording::over(tier.memory());
    let instances = instantiate::instantiate(graph, target, config.rate)?;
    let root = instances.instance_of(target)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&root))?;
    let keys = keys(graph, &instances, &order, &config);
    let mut found = frontier::Frontier::from((&instances, &order), &keys, &root, &config);
    found.walked(tier, round).await;
    if keep == Keep::Nothing && found.stored.contains_key(&root) {
        recording.found(found.lookups);
        return Ok(Reached::Held(Box::new(recording.stats())));
    }
    let (mut held, typed) = loop {
        found.walked(tier, round).await;
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
                let mut needs = table.needs(range);
                while !needs.is_empty() {
                    let fetched = tier.fetch(&needs).await;
                    for (key, parts) in fetched.handed {
                        table.took(key, &parts);
                    }
                    needs = fetched.left;
                }
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
    let mut offers = Offers::of(&mut held, (&keys, &found), tier.memory());
    recording.found(std::mem::take(&mut found.lookups));
    let walked = recording.stats();
    match driving(&mut held, recording)? {
        Some(mut driver) => {
            while driver.pull()? {
                offers.whole(&driver.table, tier.memory());
                let needs = driver.table.needs(driver.next());
                for (key, parts) in tier.fetch(&needs).await.handed {
                    driver.table.took(key, &parts);
                }
            }
            offers.rest(&driver.table, tier.memory());
            drove(&mut held, driver, keep == Keep::Wanted);
        }
        None => held.cache_stats = Some(walked),
    }
    let computed = held.table.as_ref().map_or(Vec::new(), |table| {
        let computed = table.values.iter();
        let computed = computed.filter(|(_, v)| !matches!(v.kind, table::Kind::Resident { .. }));
        computed.map(|(_, v)| v.name.clone()).collect()
    });
    if let Some(stats) = &mut held.cache_stats {
        stats.typed = typed;
        stats.planned = computed;
    }
    closed(&mut held)?;
    Ok(Reached::Render(Box::new(held)))
}

/// Each instance's node key: its source identity at the render's rate and profile.
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

/// Each node memory answered whose samples miss some its readers ask over `range`.
fn short(table: &Table, range: sva_samples::Extent) -> Vec<String> {
    let needs = table.demand(range);
    table
        .values
        .iter()
        .filter(|(at, value)| {
            matches!(value.kind, table::Kind::Resident { .. }) && !needs[*at].compute.is_empty()
        })
        .map(|(_, value)| value.name.clone())
        .collect()
}
