// Concern: renders a target over memory, from the root down to what it answers | Non-concern: what memory keeps or writes, computing a value | IO: (&Graph, target, Tier) -> Render

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use sva_ast::Graph;
use sva_formula::NodeId;

use super::value_graph;
use super::world::{Reach, Reached, Walking, World};
use super::{Render, RenderConfig, closed, driving, dropped, drove, ended, planned_over};
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

/// `target` over `tier`, from the root down, typed anew only where `session` typed it
/// otherwise; a node memory answers stands as its samples. With `out` dropped and no reading,
/// a root it answers ends the render unread. Abandoned, it stops before its next block.
pub async fn render_in<B: Backend>(
    session: &mut Session,
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier<B>,
    abandon: &dyn Abandon,
) -> Result<Render, EngineError> {
    let memory = tier.memory();
    let mut recording = Recording::over(memory);
    let Session { own, stand_in } = session;
    let (world, root) = World::rendered(own, graph, target, config.rate)?;
    let (instances, typed) = (&world.instances, &world.typing);
    let id = typed
        .id(&root)
        .ok_or_else(|| EngineError::UnknownNode(root.clone()))?;
    let (config, decided) = ended(typed, id, config);
    let lowered = typed.lowered().to_vec();
    let round = tier.begin();
    let found = walked(world, (&root, &config), (tier, round, &mut recording)).await;
    let tys = typed.clone();
    if dropped(&config) && found.held.contains_key(&root) {
        let schedule = schedule::plan(&tys, id, &config.asks);
        let mut held = Render::shell((tys, id), (config, schedule), memory.clone());
        let mut stats = recording.stats(memory);
        stats.typed = lowered;
        held.cache_stats = Some(stats);
        return Ok(held);
    }
    let hits: BTreeMap<NodeId, Arc<Stored>> = found
        .held
        .iter()
        .filter_map(|(path, stored)| Some((tys.id(path)?, Arc::clone(stored))))
        .collect();
    let mut held = planned_over(
        (graph, target, instances),
        (tys, id),
        (config, &mut *stand_in, memory.clone()),
        (decided, &hits),
    )?;
    let mut retyped = lowered;
    retyped.append(&mut held.stand_in_typed);
    if let (Some(value_graph), Some(range)) = (&mut held.value_graph, held.range) {
        loop {
            let mut needs = value_graph.needs(range);
            while !needs.is_empty() {
                let fetched = tier.fetch(&needs).await;
                for (key, parts) in fetched.handed {
                    value_graph.took(key, &parts);
                }
                needs = fetched.left;
            }
            let short = value_graph.short((value_graph.root, range, value_graph::Past::Held));
            if short.is_empty() {
                break;
            }
            for at in short {
                value_graph.read_on(&held.tys, at)?;
            }
            value_graph.settled(value_graph.root, &[], (range.start, false));
            value_graph.refuse_endless(range)?;
        }
        value_graph.offers(&held.tys, range);
    }
    let walked = recording.stats(memory);
    match driving(&mut held, (memory, recording))? {
        Some(mut driver) => {
            loop {
                if abandon.abandoned().await {
                    return Err(EngineError::Abandoned);
                }
                if !driver.pull()? {
                    break;
                }
                let needs = driver.value_graph.needs(driver.next());
                for (key, parts) in tier.fetch(&needs).await.handed {
                    driver.value_graph.took(key, &parts);
                }
            }
            drove(&mut held, driver);
        }
        None => held.cache_stats = Some(walked),
    }
    let computed = held.value_graph.as_ref().map_or(Vec::new(), |value_graph| {
        let computed = value_graph.values.iter();
        let computed =
            computed.filter(|(_, v)| !matches!(v.kind, value_graph::Kind::Resident { .. }));
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
    (root, config): (&str, &RenderConfig),
    (tier, round, seen): (&Tier<B>, u64, &mut Recording),
) -> Reached {
    let walking = Walking {
        root,
        config,
        whole: true,
    };
    loop {
        let memory = tier.memory();
        match world.walk(&walking, &mut |node, key| {
            memory.answered((key, round), (node, seen))
        }) {
            Reach::Reached(reached) => return reached,
            Reach::Asks(keys) => {
                for key in keys {
                    tier.lookup(key, round).await;
                }
            }
        }
    }
}
