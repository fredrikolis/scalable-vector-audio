// Concern: renders a target over memory, from the root down to what it answers | Non-concern: what memory keeps or writes, computing a value | IO: (&Graph, target, Tier) -> Render

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;

use sva_ast::Graph;
use sva_formula::{Hash, NodeId};

use super::offer::{Offers, readable};
use super::table::{self, Table};
use super::world::{Reach, Reached, Walking, World};
use super::{Render, RenderConfig, closed, driving, dropped, drove, planned_over};
use crate::cache::{Backend, Recording, Stored, Tier};
use crate::error::EngineError;
use crate::schedule;

/// Whether a render's caller let it go, asked between blocks.
pub trait Abandon {
    fn abandoned(&self) -> Pin<Box<dyn Future<Output = bool> + '_>>;
}

pub struct Never;

/// Renders made in turn, each advancing the version the last held.
#[derive(Default)]
pub struct Session {
    own: Option<World>,
    /// The composition with every volatile parameter at its stand-in.
    pub(super) stand_in: Option<World>,
}

impl Abandon for Never {
    fn abandoned(&self) -> Pin<Box<dyn Future<Output = bool> + '_>> {
        Box::pin(std::future::ready(false))
    }
}

/// `target` over `tier`, from the root down, in a session of its own.
pub async fn render_over<B: Backend>(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
) -> Result<Render, EngineError> {
    render_in(&mut Session::default(), graph, target, config, tier, &Never).await
}

/// `target` over `tier`, from the root down: every node is named by what it computes, typed
/// anew only where `session` typed it otherwise, and a node memory answers stands as its
/// samples, nothing under it planned or computed; with `out` dropped and no reading, a root it
/// answers ends the render unread. Abandoned, it stops before its next block.
pub async fn render_in<B: Backend>(
    session: &mut Session,
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
    abandon: &dyn Abandon,
) -> Result<Render, EngineError> {
    let round = tier.begin();
    let mut recording = Recording::over(tier.memory());
    let Session { own, stand_in } = session;
    let (world, root) = World::rendered(own, graph, target, config.rate)?;
    let (instances, typed) = (&world.instances, &world.typing);
    let order = schedule::schedule_from(instances, std::slice::from_ref(&root))?;
    let lowered = typed.lowered().to_vec();
    let keys = keys(world, &order, &config);
    let mut opened = BTreeSet::new();
    let mut found = walked(world, (&root, &config, &opened), (tier, round)).await;
    let stood = |stored: &BTreeMap<String, Arc<Stored>>| {
        let mut tys = typed.clone();
        tys.stand(stored);
        tys
    };
    if dropped(&config) && found.held.contains_key(&root) {
        recording.found(std::mem::take(&mut found.lookups));
        let tys = stood(&found.held);
        let id = tys.id(&root).ok_or(EngineError::UnknownNode(root))?;
        let schedule = schedule::plan(&tys, id, &config.asks);
        let mut held = Render::shell(tys, id, config, schedule);
        let mut stats = recording.stats();
        stats.typed = lowered;
        held.cache_stats = Some(stats);
        return Ok(held);
    }
    let mut retyped = lowered;
    let mut held = loop {
        let tys = stood(&found.held);
        let id = tys
            .id(&root)
            .ok_or_else(|| EngineError::UnknownNode(root.clone()))?;
        let bounds: BTreeSet<NodeId> = keys
            .keys()
            .filter_map(|path| tys.id(path))
            .filter(|id| readable(&tys, *id))
            .collect();
        let mut held = planned_over(
            (graph, target, instances),
            (tys, id),
            (config.clone(), &mut *stand_in),
            &bounds,
        )?;
        retyped.append(&mut held.stand_in_typed);
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
        opened.extend(short);
        found = walked(world, (&root, &config, &opened), (tier, round)).await;
    };
    let mut offers = Offers::of(&mut held, (&keys, &found, &order), tier.memory());
    recording.found(std::mem::take(&mut found.lookups));
    let walked = recording.stats();
    match driving(&mut held, recording)? {
        Some(mut driver) => {
            loop {
                if abandon.abandoned().await {
                    return Err(EngineError::Abandoned);
                }
                if !driver.pull()? {
                    break;
                }
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
        stats.typed = retyped;
        stats.planned = computed;
        stats.unslotted = held.unslotted.clone();
    }
    closed(&mut held)?;
    Ok(held)
}

/// What memory answers from `root` down, each key it cannot answer looked up this round.
async fn walked<B: Backend>(
    world: &World,
    (root, config, opened): (&str, &RenderConfig, &BTreeSet<String>),
    (tier, round): (&Tier<B>, u64),
) -> Reached {
    let walking = Walking {
        root,
        config,
        whole: true,
        opened,
    };
    loop {
        let memory = tier.memory();
        match world.walk(&walking, &|key| memory.answer(key, round)) {
            Reach::Reached(reached) => return reached,
            Reach::Asks(keys) => {
                for key in keys {
                    tier.lookup(key, round).await;
                }
            }
        }
    }
}

/// Each instance's node key; an instance whose identity refuses is never looked up.
fn keys(world: &World, order: &schedule::Order, config: &RenderConfig) -> BTreeMap<String, Hash> {
    let paths = order.groups.iter().flatten();
    let named = paths.filter_map(|path| Some((path.clone(), world.key(path, config)?)));
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
