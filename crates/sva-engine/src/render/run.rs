// Concern: renders a target over memory, from the root down to what it answers | Non-concern: what memory keeps or writes, computing a value | IO: (&Graph, target, Tier) -> Render

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;
use sva_formula::{Hash, NodeId};

use super::offer::{Offers, readable};
use super::table::{self, Table};
use super::{Render, RenderConfig, closed, driving, dropped, drove, frontier, planned_over};
use crate::cache::{Backend, Recording, Tier};
use crate::error::EngineError;
use crate::instantiate;
use crate::schedule;
use crate::typing;

/// `target` over `tier`, from the root down: a node memory answers stands as its samples, and
/// nothing under it is typed, planned or looked up; with `out` dropped and no reading, a root
/// it answers ends the render unread. What the rest computes memory keeps as it says.
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
    let keys = keys(graph, &instances, &order, &config);
    let mut found = frontier::Frontier::from((&instances, &order), &keys, &root, &config);
    found.walked(tier, round).await;
    if dropped(&config) && found.stored.contains_key(&root) {
        recording.found(std::mem::take(&mut found.lookups));
        let tys = typing::infer_over(&instances, &order.within(&found.visited), &found.stored)?;
        let id = tys.id(&root).ok_or(EngineError::UnknownNode(root))?;
        let schedule = schedule::plan(&tys, id, &config.asks);
        let mut held = Render::shell(tys, id, config, schedule);
        held.cache_stats = Some(recording.stats());
        return Ok(held);
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
        stats.typed = typed;
        stats.planned = computed;
    }
    closed(&mut held)?;
    Ok(held)
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
