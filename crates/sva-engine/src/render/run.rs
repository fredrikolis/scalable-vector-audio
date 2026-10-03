// Concern: renders a target over memory, from the root down to what it answers | Non-concern: what memory keeps or writes, computing a value | IO: (&Graph, target, Tier) -> Render

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use sva_ast::Graph;
use sva_formula::{Hash, NodeId};

use super::offer::{Offers, readable};
use super::table::{self, Table};
use super::{Render, RenderConfig, closed, driving, dropped, drove, frontier, planned_over};
use crate::cache::{Backend, Recording, Stored, Tier};
use crate::error::EngineError;
use crate::instantiate;
use crate::schedule;
use crate::typing::{self, Typing};

/// `target` over `tier`, from the root down: every node is typed and named by what it
/// computes, and a node memory answers stands as its samples, nothing under it planned, looked
/// up or computed; with `out` dropped and no reading, a root it answers ends the render
/// unread. What the rest computes memory keeps as it says.
pub async fn render_over<B: Backend>(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
) -> Result<Render, EngineError> {
    let round = tier.begin();
    let mut recording = Recording::over(tier.memory());
    let instances = instantiate::instantiate(graph, target, config.rate)?;
    let root = instances.instance_of(target)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&root))?;
    let typed = typing::infer_all(&instances, &order)?;
    let lowered = typed.lowered().to_vec();
    let keys = keys(&typed, &order, &config);
    let mut fresh = Some(typed);
    let mut found = frontier::Frontier::from((&instances, &order), &keys, &root, &config);
    found.walked(tier, round).await;
    let mut stood = |stored: &BTreeMap<String, Arc<Stored>>| {
        let typed = fresh
            .take()
            .map_or_else(|| typing::infer_all(&instances, &order), Ok);
        typed.map(|mut tys| {
            tys.stand(stored);
            tys
        })
    };
    if dropped(&config) && found.stored.contains_key(&root) {
        recording.found(std::mem::take(&mut found.lookups));
        let tys = stood(&found.stored)?;
        let id = tys.id(&root).ok_or(EngineError::UnknownNode(root))?;
        let schedule = schedule::plan(&tys, id, &config.asks);
        let mut held = Render::shell(tys, id, config, schedule);
        let mut stats = recording.stats();
        stats.typed = lowered;
        held.cache_stats = Some(stats);
        return Ok(held);
    }
    let mut held = loop {
        found.walked(tier, round).await;
        let tys = stood(&found.stored)?;
        let id = tys
            .id(&root)
            .ok_or_else(|| EngineError::UnknownNode(root.clone()))?;
        let bounds: BTreeSet<NodeId> = keys
            .keys()
            .filter_map(|path| tys.id(path))
            .filter(|id| readable(&tys, *id))
            .collect();
        let mut held = planned_over(
            (graph, target, &instances),
            (tys, id),
            config.clone(),
            &bounds,
        )?;
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
            break held;
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
            drove(&mut held, driver);
        }
        None => held.cache_stats = Some(walked),
    }
    let computed = held.table.as_ref().map_or(Vec::new(), |table| {
        let computed = table.values.iter();
        let computed = computed.filter(|(_, v)| !matches!(v.kind, table::Kind::Resident { .. }));
        computed.map(|(_, v)| v.name.clone()).collect()
    });
    if let Some(stats) = &mut held.cache_stats {
        stats.typed = lowered;
        stats.planned = computed;
        stats.unslotted = held.unslotted.clone();
    }
    closed(&mut held)?;
    Ok(held)
}

/// Each instance's node key: what it computes, at the render's rate and profile; an instance
/// whose identity refuses is never looked up.
pub(crate) fn keys(
    tys: &Typing,
    order: &schedule::Order,
    config: &RenderConfig,
) -> BTreeMap<String, Hash> {
    let paths = order.groups.iter().flatten();
    let named = paths.filter_map(|path| {
        let identity = crate::refs::identity(tys, tys.id(path)?).ok()?;
        let key = crate::cache::node_key(identity, config.rate, &config.profile);
        Some((path.clone(), key))
    });
    named.collect()
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
