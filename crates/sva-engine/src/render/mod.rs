// Concern: holds a composition's decided types, the buffers a reading needed and each label | Non-concern: computing a value (table/), what a reading asks (schedule.rs) | IO: (&Graph, target) -> Render

mod answer;
mod drive;
mod end;
mod run;
mod slots;
mod stream;
pub(crate) mod table;
mod terms;
pub mod until;
mod volatile;
mod world;

use std::collections::BTreeMap;
use std::sync::Arc;

use sva_ast::Graph;
use sva_formula::{NodeId, SpectralSum};
use sva_samples::{Buffer, Extent, FilterTrace, Frames, Label, PSYCHOACOUSTIC_V1, Profile};

use crate::bindings::Binding;
use crate::cache::{CacheStats, Memory, Recording, Stored, Tier, now};
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate;
use crate::query::Ask;
use crate::refs;
use crate::schedule::{self, Schedule};
use crate::typing::{self, Typing};
use end::Ending;
use table::Table;
use table::support::Supports;

/// Where a target is read, in samples from t = 0. Unstated, the root's own support.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    pub rate: u32,
    pub range: Range,
    /// Where the render stops before the range ends.
    pub until: Option<Until>,
    pub profile: Profile,
    /// What the caller means to read. An empty list is audio out, which collapses the root.
    pub asks: Vec<Ask>,
    /// The operation count this render may pay in place of its profile's.
    pub flop_budget: Option<u128>,
    /// What reads one of these keeps one value in memory, its last.
    pub volatile: Vec<String>,
    pub out: Out,
}

/// Whether a render hands back its root's samples; dropped, with no reading asked, it holds
/// none past each block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Out {
    #[default]
    Kept,
    Dropped,
}

impl RenderConfig {
    pub fn budget(&self) -> u128 {
        self.flop_budget.unwrap_or(self.profile.flop_budget)
    }

    pub fn at(rate: u32) -> RenderConfig {
        RenderConfig {
            rate,
            range: Range::default(),
            until: None,
            profile: PSYCHOACOUSTIC_V1,
            asks: Vec::new(),
            flop_budget: None,
            volatile: Vec::new(),
            out: Out::Kept,
        }
    }

    pub fn seconds(rate: u32, secs: f64) -> RenderConfig {
        let end = (secs * f64::from(rate)).round() as i64;
        RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(end),
            },
            ..RenderConfig::at(rate)
        }
    }

    pub fn asking(mut self, asks: Vec<Ask>) -> RenderConfig {
        self.asks = asks;
        self
    }
}

pub use answer::{answer, answer_buffer, sketch_atom};
pub use run::{Abandon, Never, Session, render_in, render_over};
pub use stream::{
    Built, Change, Changed, Counts, LATEST, Placed, Stream, StreamConfig, change, fetch,
};
pub use terms::{Handle, NOTES};
pub use until::Until;
pub use world::STREAMED;

/// Samples a whole render pulls at once; any size writes the same bits.
const BLOCK: usize = 1 << 12;

pub struct Render {
    pub root: NodeId,
    pub tys: Typing,
    pub buffers: BTreeMap<NodeId, Buffer>,
    pub frames: BTreeMap<NodeId, Frames>,
    pub symbolic: BTreeMap<NodeId, SpectralSum>,
    pub labels: BTreeMap<NodeId, Label>,
    pub traces: Vec<FilterTrace>,
    pub config: RenderConfig,
    pub schedule: Schedule,
    pub bindings: BTreeMap<NodeId, Vec<Binding>>,
    pub cache_stats: Option<CacheStats>,
    /// The most bytes the table held at once, samples and state.
    pub held_bytes: usize,
    /// The samples the root was read over; `None` where no reading needed any.
    pub range: Option<Extent>,
    pub(crate) unranged: Option<EngineError>,
    /// Every value the render read, each over the segments it computed.
    pub(crate) table: Option<Table>,
    /// The price of what its pulls computed.
    computed: u128,
    pub(crate) unslotted: Option<String>,
    /// Each node typed with every volatile parameter at its stand-in.
    pub(crate) stand_in_typed: Vec<String>,
    pub(crate) cut: Option<i64>,
    /// What its readings ask past the samples it holds.
    pub(crate) memory: Memory,
}

impl Render {
    pub(crate) fn shell(
        (tys, root): (Typing, NodeId),
        (config, schedule): (RenderConfig, Schedule),
        memory: Memory,
    ) -> Self {
        Render {
            root,
            tys,
            buffers: BTreeMap::new(),
            frames: BTreeMap::new(),
            symbolic: BTreeMap::new(),
            labels: BTreeMap::new(),
            traces: Vec::new(),
            config,
            schedule,
            bindings: BTreeMap::new(),
            cache_stats: None,
            held_bytes: 0,
            range: None,
            unranged: None,
            table: None,
            computed: 0,
            unslotted: None,
            stand_in_typed: Vec::new(),
            cut: None,
            memory,
        }
    }

    /// What it computed: nothing memory answered is priced.
    pub fn work(&self) -> crate::flops::Work {
        crate::flops::Work {
            samples: self.range.map_or(0, |range| range.len() as u64),
            priced_flops: self.computed,
            waves: None,
        }
    }

    /// A node's samples over the range, zero where it holds none.
    pub fn output(&self, node: NodeId) -> Result<Buffer, EngineError> {
        match (self.buffers.get(&node), self.range) {
            (Some(held), Some(range)) => Ok(held.over(range, held.extent())),
            _ => Err(answer::unheld(self, node)),
        }
    }

    pub fn buffer(&self, node: NodeId) -> Option<&Buffer> {
        self.buffers.get(&node)
    }

    pub fn id(&self, path: &str) -> Option<NodeId> {
        self.tys.id(path)
    }

    /// The node a reading names, refusing by name where a file expanded into several.
    pub fn node(&self, path: &str) -> Result<NodeId, EngineError> {
        self.tys.resolve(path)
    }

    /// Each segment the render computed of `node`'s value, in order: a sample in two was
    /// computed twice.
    pub fn evaluated(&self, node: NodeId) -> Vec<Extent> {
        let Some(table) = &self.table else {
            return Vec::new();
        };
        table
            .of(node)
            .map_or(Vec::new(), |at| table.values[at].evaluated.clone())
    }

    /// The multiple a reading asks this node's score against, where one asks for a score at all.
    pub fn alias_oversample(&self, node: NodeId) -> Option<u32> {
        self.config
            .asks
            .iter()
            .find_map(|ask| match ask.representation {
                crate::query::Representation::Alias { oversample }
                    if self.node(&ask.node).is_ok_and(|asked| asked == node) =>
                {
                    Some(oversample)
                }
                _ => None,
            })
    }
}

/// `render_over` memory alone. Nothing is materialized that no reading asked for.
pub fn render(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    tier: &Tier,
) -> Result<Render, EngineError> {
    now(render_over(graph, target, config, tier))
}

fn closed(held: &mut Render) -> Result<(), EngineError> {
    scored(held)?;
    compose_read(held);
    stamp(held);
    Ok(())
}

pub(crate) struct Prepared {
    pub(crate) instances: instantiate::Instances,
    pub(crate) tys: Typing,
    pub(crate) root: NodeId,
}

pub(crate) fn prepared(graph: &Graph, target: &str, rate: u32) -> Result<Prepared, EngineError> {
    let instances = instantiate::instantiate(graph, target, rate)?;
    let held = instances.instance_of(target)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&held))?;
    let tys = typing::infer_all(&instances, &order)?;
    let root = tys
        .id(&held)
        .ok_or_else(|| EngineError::UnknownNode(held.clone()))?;
    Ok(Prepared {
        instances,
        tys,
        root,
    })
}

fn planned(
    (graph, target): (&Graph, &str),
    prepared: Prepared,
    (config, memory): (RenderConfig, Memory),
) -> Result<Render, EngineError> {
    let Prepared {
        instances,
        tys,
        root,
    } = prepared;
    let config = (config, &mut None, memory);
    let none = (None, &BTreeMap::new());
    planned_over((graph, target, &instances), (tys, root), config, none)
}

fn planned_over(
    (graph, target, instances): (&Graph, &str, &instantiate::Instances),
    (tys, root): (Typing, NodeId),
    (config, stand_in, memory): (RenderConfig, &mut Option<world::World>, Memory),
    (decided, hits): (Option<end::End>, &BTreeMap<NodeId, Arc<Stored>>),
) -> Result<Render, EngineError> {
    let schedule = schedule::plan(&tys, root, &config.asks);
    let bindings = tys
        .paths()
        .filter_map(|(path, id)| Some((id, resolved(instances, path)?)))
        .collect();
    let mut held = Render::shell((tys, root), (config, schedule), memory);
    held.bindings = bindings;
    ranged(&mut held, (decided, hits))?;
    let volatile = volatile::mark((graph, instances), &held, (target, stand_in))?;
    if let Some(table) = &mut held.table {
        table.slots(volatile.slots());
    }
    held.unslotted = volatile.unslotted.clone();
    held.stand_in_typed = volatile.typed;
    Ok(held)
}

/// The range and every value a render of `target` would compute, and no sample.
pub fn plan(graph: &Graph, target: &str, config: RenderConfig) -> Result<Render, EngineError> {
    planned(
        (graph, target),
        prepared(graph, target, config.rate)?,
        (config, Memory::holding(0)),
    )
}

/// The sample each of `roots` read bare would end at, `None` where its support never ends.
pub fn ends(
    graph: &Graph,
    roots: &[String],
    config: &RenderConfig,
) -> Result<Vec<Option<i64>>, EngineError> {
    let (instances, named) = instantiate::from_roots(graph, roots, config.rate)?;
    let order = schedule::schedule_from(&instances, &named)?;
    let tys = typing::infer_all(&instances, &order)?;
    let supports = Supports::new(&tys);
    let ending = Ending::new(&tys, &config.profile, &supports);
    named
        .iter()
        .map(|held| {
            let id = tys
                .id(held)
                .ok_or_else(|| EngineError::UnknownNode(held.clone()))?;
            Ok(default_end(ending.of(id).support))
        })
        .collect()
}

/// An open render's end, found on what each node computes, never on what memory answers.
pub(crate) fn ended(
    tys: &Typing,
    root: NodeId,
    mut config: RenderConfig,
) -> (RenderConfig, Option<end::End>) {
    if config.range.end.is_some() {
        return (config, None);
    }
    let supports = Supports::new(tys);
    let end = Ending::new(tys, &config.profile, &supports).of(root);
    config.range.end = default_end(end.support);
    (config, Some(end))
}

/// A reading of samples or of their cost needs the range; lines and structure never do.
/// `decided`: where an open render's root ends, found before it was planned.
fn ranged(
    held: &mut Render,
    (decided, hits): (Option<end::End>, &BTreeMap<NodeId, Arc<Stored>>),
) -> Result<(), EngineError> {
    let counts = counts(&held.config.asks);
    let envelope = held.config.asks.iter().any(|ask| {
        matches!(
            ask.representation,
            crate::query::Representation::Envelope { .. }
        )
    });
    let tabled = materializes(held) || counts;
    if !tabled && !envelope {
        return Ok(());
    }
    let supports = Supports::new(&held.tys);
    let end = match (held.config.range.end, decided) {
        (_, Some(end)) => end,
        (Some(_), None) => Ending::new(&held.tys, &held.config.profile, &supports).exact(held.root),
        (None, None) => Ending::new(&held.tys, &held.config.profile, &supports).of(held.root),
    };
    held.cut = end.cut;
    let support = end.support;
    let range = range_over(
        (&held.config, held.tys.name(held.root)),
        support,
        Ends::Refused,
    );
    if !tabled {
        match range {
            Ok(range) => held.range = Some(range),
            Err(refused) => held.unranged = Some(refused),
        }
        return Ok(());
    }
    held.range = Some(range?);
    let found = supports.into_memo();
    let wanted: Vec<NodeId> = held.schedule.wanted.clone();
    let root = (held.root, wanted.as_slice());
    let mut table = Table::bounded(&held.tys, root, &held.config.profile, (found, hits))?;
    table.plan(held.range.expect("a range was decided"))?;
    held.table = Some(table);
    Ok(())
}

fn materializes(held: &Render) -> bool {
    !held.schedule.wanted.is_empty()
}

pub(crate) enum Ends {
    Refused,
    Pulled,
}

/// The range a root of `support` is read over.
pub(crate) fn range_over(
    (config, name): (&RenderConfig, &str),
    support: Extent,
    ends: Ends,
) -> Result<Extent, EngineError> {
    let start = config.range.start.unwrap_or_else(|| default_start(support));
    let end = match config.range.end.or(default_end(support)) {
        Some(end) => end,
        None => match ends {
            Ends::Pulled => i64::MAX,
            Ends::Refused => return Err(endless(name)),
        },
    };
    Ok(Extent::new(start, end.max(start)))
}

fn default_start(support: Extent) -> i64 {
    match support.is_empty() || support.start == i64::MIN || support.start > 0 {
        true => 0,
        false => support.start,
    }
}

fn default_end(support: Extent) -> Option<i64> {
    (support.end != i64::MAX).then_some(support.end)
}

fn endless(name: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "render.no_end".to_string(),
        message: format!(
            "`{name}` is read over an interval with no end, and its support never ends"
        ),
        location: Located::at(name, None),
        help: "give the interval an end, as `[0, 2s]`, or crop it".to_string(),
    })
}

/// A `flops` reading counts what the audio render would run.
fn counts(asks: &[Ask]) -> bool {
    asks.iter()
        .any(|ask| ask.representation == crate::query::Representation::Flops)
}

/// A reading that materializes nothing is never refused for cost; a count past the budget
/// refuses before a sample is computed.
fn affordable(held: &Render) -> Result<(), EngineError> {
    if !materializes(held) {
        return Ok(());
    }
    let total = crate::flops::total(held);
    if total <= held.config.budget() {
        return Ok(());
    }
    let counted = crate::flops::tree(held);
    let over = crate::flops::dominating(&counted).expect("a counted tree holds its root");
    Err(EngineError::refused(Diagnostic {
        code: "collapse.over_budget".to_string(),
        message: format!(
            "this render counts {} operations, over the budget of {}; `{}` dominates it at {} \
             by {}",
            counted.total, counted.budget, over.node, over.subtree, over.route
        ),
        location: Located::at(held.tys.name(held.root), None),
        help: format!("pass --flop-budget {total} to render it anyway"),
    }))
}

/// Every wanted value pulled over the range, block by block, until `until` stops it.
#[cfg(test)]
fn pulled(held: &mut Render, memo: (&Memory, Recording)) -> Result<(), EngineError> {
    if let Some(mut driver) = driving(held, memo)? {
        while driver.pull()? {}
        drove(held, driver);
    }
    Ok(())
}

/// What pulls a render's table, where it materializes one at all.
fn driving(
    held: &mut Render,
    (memory, recording): (&Memory, Recording),
) -> Result<Option<drive::Driver>, EngineError> {
    affordable(held)?;
    let (Some(table), Some(range)) = (held.table.take(), held.range) else {
        return Ok(None);
    };
    if !materializes(held) {
        held.table = Some(table);
        return Ok(None);
    }
    Ok(Some(drive::Driver::new(
        table,
        range,
        BLOCK,
        &held.config,
        (memory.clone(), recording),
    )))
}

pub(crate) fn dropped(config: &RenderConfig) -> bool {
    config.out == Out::Dropped && config.asks.is_empty()
}

fn drove(held: &mut Render, driver: drive::Driver) {
    let keep = !dropped(&held.config);
    let range = held.range.expect("a pulled render has a range");
    held.held_bytes = driver.most_bytes();
    held.computed = driver.work.priced_flops;
    held.cache_stats = Some(driver.recording.stats(&driver.memory));
    if let Some(stop) = driver.stop().filter(|stop| *stop < range.end) {
        held.range = Some(Extent::new(range.start, stop));
    }
    let range = held.range.expect("a pulled render has a range");
    let table = driver.table;
    let wanted = held.schedule.wanted.iter().filter(|_| keep);
    for (id, at) in wanted.map(|id| (*id, table.of(*id))) {
        let Some(at) = at else {
            continue;
        };
        match &table.values[at].held {
            table::Held::Frames(Some(frames)) => {
                held.frames.insert(id, (**frames).clone());
            }
            _ => {
                held.buffers.insert(id, table.samples(at, range));
                held.labels.insert(id, table.label(at));
            }
        }
    }
    held.table = Some(table);
}

/// A closed form's value `fine` times finer than the render's step: what an alias score
/// reads against.
pub(crate) fn finer(
    render: &Render,
    node: NodeId,
    fine: u32,
    over: Extent,
) -> Result<Buffer, EngineError> {
    let profile = &render.config.profile;
    let mut table = Table::finer(&render.tys, node, &[node], profile, i128::from(fine))?;
    let at = table.root;
    let memory = &render.memory;
    table.pull(over, (memory, &mut Recording::over(memory)))?;
    let mut held = table.samples(at, over);
    held.rate = render.config.rate * fine;
    Ok(held)
}

/// A point sampling a reading asks the score of says in its label what the grid lost.
fn scored(held: &mut Render) -> Result<(), EngineError> {
    let asked: Vec<NodeId> = held
        .labels
        .keys()
        .copied()
        .filter(|id| held.alias_oversample(*id).is_some())
        .collect();
    for id in asked {
        let sva_samples::Detail::Point {
            rule,
            alias_db: None,
            tail_db,
        } = held.labels[&id].detail
        else {
            continue;
        };
        let buffer = held.output(id)?;
        let alias_db = Some(answer::alias_db(held, id, &buffer)?);
        let label = held.labels.get_mut(&id).expect("an asked label");
        label.detail = sva_samples::Detail::Point {
            rule,
            alias_db,
            tail_db,
        };
    }
    Ok(())
}

/// `render` with every read its own value, over a memory keeping none.
#[cfg(test)]
pub(crate) fn render_apart(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
) -> Result<Render, EngineError> {
    let mut held = planned(
        (graph, target),
        prepared(graph, target, config.rate)?,
        (config, Memory::holding(0)),
    )?;
    if let (Some(range), Some(_)) = (held.range, &held.table) {
        let wanted = held.schedule.wanted.clone();
        let mut table = Table::apart(&held.tys, held.root, &wanted, &held.config.profile)?;
        table.plan(range)?;
        held.table = Some(table);
    }
    let memory = held.memory.clone();
    pulled(&mut held, (&memory, Recording::over(&memory)))?;
    Ok(held)
}

pub(crate) fn sampled(render: &Render, node: NodeId, over: Extent) -> Result<Buffer, EngineError> {
    let mut table = Table::build(&render.tys, node, &[node], &render.config.profile)?;
    let at = table.root;
    let memory = &render.memory;
    table.pull(over, (memory, &mut Recording::over(memory)))?;
    Ok(table.samples(at, over))
}

/// FORMAT 9.3: the render's own label says what it cost and what it was allowed.
fn stamp(held: &mut Render) {
    let root = held.root;
    let Some(label) = held.labels.remove(&root) else {
        return;
    };
    let counted = crate::flops::total(held);
    let cut = held.cut.map(|at| (held.tys.name(root).to_string(), at));
    let pruned = sva_samples::Pruned {
        db: held.config.profile.prune_db,
        cuts: cut.into_iter().collect(),
    };
    let label = sva_samples::Label {
        rate: held.config.rate,
        moved: held.table.as_ref().map(Table::moved),
        pruned: Some(pruned),
        ..label.costing(counted, held.config.budget())
    };
    held.labels.insert(root, label);
}

/// One render, every reading: a closed form a reading asks for is composed once here; one
/// that refuses is left for the reading to raise.
fn compose_read(held: &mut Render) {
    for id in held.schedule.compose.clone() {
        if held.symbolic.contains_key(&id) {
            continue;
        }
        if let Ok(sum) = refs::spectral_sum_of(&held.tys, id, held.tys.var(id)) {
            held.symbolic.insert(id, sum);
        }
    }
}

fn resolved(instances: &instantiate::Instances, path: &str) -> Option<Vec<Binding>> {
    Some(
        instances
            .bindings(path)?
            .into_iter()
            .map(|(name, expr, cx)| Binding {
                name: name.to_string(),
                source: instances.render(expr, cx),
            })
            .collect(),
    )
}
