// Concern: holds a composition's decided types, the buffers a reading needed and each label | Non-concern: ordering the work (schedule.rs), a sampled shader (sampled.rs) | IO: (&Graph, target) -> Render

mod answer;
pub(crate) mod extent;
mod pointwise;
mod reach;
mod sampled;
mod silent;
mod slots;
mod stream;
pub mod until;
mod volatile;

use std::collections::BTreeMap;

use sva_ast::Graph;
use sva_formula::{Held, NodeId, SpectralSum, hash_closed_form};
use sva_samples::{
    AliasScore, Buffer, Extent, FilterTrace, Frames, Label, PSYCHOACOUSTIC_V1, Profile, stft,
};

use crate::bindings::Binding;
use crate::cache::{
    Cache, CachePolicy, CacheStats, Expected, Lens, Payload, Recording, frames_key, symbolic_key,
};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate;
use crate::query::Ask;
use crate::refs;
use crate::schedule::{self, Schedule};
use crate::typing::{self, Typing, Value};

/// Where a target is read, grid samples from sample 0 at t = 0. An unstated start is where
/// the root starts, before t = 0 only where its support reaches there; an unstated end is
/// where `until` is proven to hold.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

pub const DEFAULT_PROOF_LIMIT_SECS: f64 = 60.0;

#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    pub rate: u32,
    pub range: Range,
    /// Where the render ends before the range does; `None` reads the range to its end.
    pub until: Option<Until>,
    pub proof_limit_secs: f64,
    pub profile: Profile,
    /// What the caller means to read. An empty list is audio out, which collapses the root.
    pub asks: Vec<Ask>,
    /// The operation count this render may pay; the profile's own until a caller raises it.
    pub flop_budget: u128,
    /// What reads one of these keeps one value in the store, its last.
    pub volatile: Vec<String>,
    /// The store's own where `None`.
    pub cache_policy: Option<CachePolicy>,
}

impl RenderConfig {
    pub fn at(rate: u32) -> RenderConfig {
        RenderConfig {
            rate,
            range: Range::default(),
            until: None,
            proof_limit_secs: DEFAULT_PROOF_LIMIT_SECS,
            profile: PSYCHOACOUSTIC_V1,
            asks: Vec::new(),
            flop_budget: PSYCHOACOUSTIC_V1.flop_budget,
            volatile: Vec::new(),
            cache_policy: None,
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
pub use stream::{Block, Checkpoint, STREAMED, Stream, StreamConfig};
pub use until::Until;

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
    pub proofs: u64,
    /// The samples the root was read over; `None` where no reading needed any.
    pub range: Option<Extent>,
    pub(crate) unranged: Option<EngineError>,
    pub(crate) extents: extent::Extents,
}

impl Render {
    pub(crate) fn shell(
        tys: Typing,
        root: NodeId,
        config: RenderConfig,
        schedule: Schedule,
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
            proofs: 0,
            range: None,
            unranged: None,
            extents: extent::Extents::default(),
        }
    }

    pub fn work(&self) -> crate::flops::Work {
        crate::flops::Work {
            samples: self.range.map_or(0, |range| range.len() as u64),
            proofs: self.proofs,
            priced_flops: crate::flops::total(self),
            waves: None,
        }
    }

    pub fn output(&self, node: NodeId) -> Option<Buffer> {
        self.buffers.contains_key(&node).then_some(())?;
        Some(self.aligned(node, self.range?))
    }

    /// A node's samples over `over`: its own where it holds them, zero outside its support.
    pub(crate) fn aligned(&self, node: NodeId, over: Extent) -> Buffer {
        self.buffers[&node].over(over, self.extents.support(node))
    }

    pub(crate) fn extent_of(&self, node: NodeId) -> Option<Extent> {
        self.extents.decided.get(&node).copied().or(self.range)
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

    pub fn alias_score(&self, node: NodeId) -> AliasScore {
        match self.alias_oversample(node) {
            Some(_) => AliasScore::Asked,
            None => AliasScore::NotAsked,
        }
    }
}

/// Nothing is materialized that no reading asked for: a closed form answered off its spectral sum
/// allocates no buffer at all.
pub fn render(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    cache: Option<&Cache>,
) -> Result<Render, EngineError> {
    let recording = cache.map(|c| Recording::over(c, config.cache_policy));
    let mut held = run(prepared(graph, target)?, config, recording.as_ref())?;
    held.cache_stats = recording.map(Recording::finish);
    Ok(held)
}

pub(crate) struct Prepared<'g> {
    pub(crate) instances: instantiate::Instances<'g>,
    pub(crate) order: schedule::Order,
    pub(crate) tys: Typing,
    pub(crate) root: NodeId,
    pub(crate) target: String,
}

impl Prepared<'_> {
    /// The root's content address, and whether a reading asks it for an alias score.
    pub(crate) fn identity(&self, asks: &[Ask]) -> Result<(sva_formula::Hash, bool), EngineError> {
        let scored = asks.iter().any(|ask| {
            matches!(
                ask.representation,
                crate::query::Representation::Alias { .. }
            ) && self.tys.resolve(&ask.node).is_ok_and(|id| id == self.root)
        });
        Ok((refs::identity(&self.tys, self.root)?, scored))
    }
}

pub(crate) fn prepared<'g>(graph: &'g Graph, target: &str) -> Result<Prepared<'g>, EngineError> {
    let instances = instantiate::instantiate(graph, target)?;
    let held = instances.instance_of(target)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&held))?;
    let tys = typing::infer_all(&instances, &order)?;
    let root = tys
        .id(&held)
        .ok_or_else(|| EngineError::UnknownNode(held.clone()))?;
    Ok(Prepared {
        instances,
        order,
        tys,
        root,
        target: target.to_string(),
    })
}

/// The range and every extent under it are decided before the first sample, the root cut
/// where `until` first holds after.
pub(crate) fn run(
    prepared: Prepared,
    config: RenderConfig,
    recording: Option<&Recording>,
) -> Result<Render, EngineError> {
    sampled::on_the_grid(&prepared.tys, config.rate)?;
    let identity = prepared.identity(&config.asks).ok();
    let Prepared {
        instances,
        order,
        tys,
        root,
        target,
    } = prepared;
    let schedule = schedule::plan(&tys, &order, root, &config.asks);
    let forks = schedule::forks(&tys, &schedule.materialize);
    let bindings = tys
        .paths()
        .filter_map(|(path, id)| Some((id, resolved(&instances, path)?)))
        .collect();
    let mut held = Render::shell(tys, root, config, schedule);
    held.bindings = bindings;
    let volatile_root = volatile::mark(&instances, &held, &target)?
        .slot(root)
        .is_some();
    let reached = reach::ranged(&mut held, recording, identity, volatile_root)?;
    let lenses = Lenses {
        recording,
        volatile: volatile::mark(&instances, &held, &target)?,
        forks,
        root,
    };
    affordable(&held)?;
    let needed = lenses.needed(&held);
    for id in held.schedule.materialize.clone() {
        if needed.contains(&id) {
            materialize(&mut held, id, &lenses)?;
        }
    }
    if let Some(reached) = reached {
        reach::stopped(&mut held, reached, recording);
    }
    compose_read(&mut held);
    stamp(&mut held);
    Ok(held)
}

/// A reading that materializes nothing is never refused for cost: asking what a render
/// would cost is not paying for it. Anything that runs is counted first, and a count past
/// the budget refuses before a sample is computed.
fn affordable(held: &Render) -> Result<(), EngineError> {
    if held.schedule.materialize.is_empty() {
        return Ok(());
    }
    let total = crate::flops::total(held);
    if total <= held.config.flop_budget {
        return Ok(());
    }
    let counted = crate::flops::tree(held);
    let over = crate::flops::dominating(&counted).expect("a counted tree holds its root");
    Err(EngineError::refused(Diagnostic {
        code: "collapse.over_budget".to_string(),
        message: format!(
            "this render counts {} operations, over the budget of {}; `{}` dominates it at \
             {} by {}",
            counted.total, counted.budget, over.node, over.subtree, over.route
        ),
        location: Located::at(held.tys.name(held.root), None),
        help: format!("pass -c flop_budget={} to render it anyway", counted.total),
    }))
}

/// FORMAT 9.3: the render's own label says what it cost and what it was allowed.
fn stamp(held: &mut Render) {
    let root = held.root;
    let Some(label) = held.labels.remove(&root) else {
        return;
    };
    let counted = crate::flops::total(held);
    held.labels
        .insert(root, label.costing(counted, held.config.flop_budget));
}

/// One render, every reading: a closed form a reading asks for is composed once here, and each
/// representation answers off that one form rather than walking the graph again. A form that
/// refuses is left for the reading itself to raise, in its own words.
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

pub(crate) struct Lenses<'r> {
    recording: Option<&'r Recording<'r>>,
    volatile: volatile::Volatile,
    forks: std::collections::BTreeSet<NodeId>,
    root: NodeId,
}

impl Lenses<'_> {
    pub(crate) fn none() -> Lenses<'static> {
        Lenses {
            recording: None,
            volatile: volatile::Volatile::default(),
            forks: Default::default(),
            root: NodeId(0),
        }
    }

    fn at(&self, id: NodeId) -> Option<Lens<'_>> {
        let fork = self.forks.contains(&id);
        self.recording
            .map(|r| r.at(self.volatile.slot(id), fork, id == self.root))
    }

    /// What a reading asks for, what the policy keeps, and what those read that the store
    /// does not answer. A ledger holds them all.
    fn needed(&self, held: &Render) -> std::collections::BTreeSet<NodeId> {
        let materialize = &held.schedule.materialize;
        let ledger = held.config.asks.iter().any(|ask| {
            matches!(
                ask.representation,
                crate::query::Representation::Ledger { .. }
            )
        });
        let kept = |id: &NodeId| {
            ledger
                || self
                    .recording
                    .is_none_or(|r| r.stores(self.forks.contains(id), *id == self.root))
        };
        let mut needed: std::collections::BTreeSet<NodeId> = materialize
            .iter()
            .copied()
            .filter(|id| held.schedule.wanted.contains(id) || kept(id))
            .collect();
        for id in materialize.iter().rev() {
            if !needed.contains(id) || self.answered(held, *id) {
                continue;
            }
            needed.extend(reads(held, *id));
        }
        needed
    }

    fn answered(&self, held: &Render, id: NodeId) -> bool {
        let (Some(recording), Held::Sampled) = (self.recording, held.tys.ty(id).held) else {
            return false;
        };
        sampled::key(held, id).is_ok_and(|(key, _)| recording.holds(key))
    }
}

/// What the schedule orders before `id`, and every buffer its program reads behind those.
fn reads(held: &Render, id: NodeId) -> Vec<NodeId> {
    let mut out = schedule::materialized_operands(&held.tys, id);
    if matches!(held.tys.ty(id).held, Held::Sampled)
        && let Ok(program) = sampled::program(held, id)
    {
        out.extend(program.reads);
    }
    out.retain(|read| *read != id);
    out
}

/// A sampled node found in the store answers for everything under it, so what it reads is
/// held here only where it missed.
fn materialize(held: &mut Render, id: NodeId, lenses: &Lenses) -> Result<(), EngineError> {
    if held.buffers.contains_key(&id) || held.frames.contains_key(&id) {
        return Ok(());
    }
    let extent = held.extents.of(id);
    if extent.is_empty() && !matches!(held.tys.ty(id).held, Held::Frames) {
        let (rate, width) = (held.config.rate, held.tys.ty(id).width as usize);
        let mut silent = Buffer::silence(rate, width.max(1), 0);
        silent.start = extent.start;
        held.buffers.insert(id, silent);
        held.labels
            .insert(id, Label::measured(held.config.profile.name, rate));
        return Ok(());
    }
    let lens = lenses.at(id);
    if matches!(held.tys.ty(id).held, Held::Sampled) {
        let (key, samples) = sampled::key(held, id)?;
        if let Some((hit, label)) = warm(held, id, key, samples, lens.as_ref()) {
            held.buffers.insert(id, hit);
            held.labels.insert(id, label);
            return Ok(());
        }
        for read in reads(held, id) {
            materialize(held, read, lenses)?;
        }
        return sampled::run(held, id, key, lens.as_ref());
    }
    for operand in schedule::materialized_operands(&held.tys, id) {
        materialize(held, operand, lenses)?;
    }
    match held.tys.ty(id).held {
        Held::Frames => frames_of(held, id, lens.as_ref()),
        _ => collapse_closed_form(held, id, lens.as_ref()),
    }
}

/// The one place a closed form becomes samples: `sample(...)` and the render root, and nothing else.
fn collapse_closed_form(
    held: &mut Render,
    id: NodeId,
    cache: Option<&Lens>,
) -> Result<(), EngineError> {
    let var = held.tys.var(id);
    let written = match refs::resolve(&held.tys, id, 0, held.tys.ty(id).held) {
        Ok(refs::Read::Substitute(form)) => Some(*form),
        _ => None,
    };
    let symbolic = written.as_ref().map(|t| symbolic_key(hash_closed_form(t)));
    let sum = remembered(held, id, symbolic, cache)
        .map(Ok)
        .unwrap_or_else(|| {
            let found = refs::spectral_sum_of(&held.tys, id, var);
            if let (Ok(sum), Some(key), Some(cache)) = (&found, symbolic, cache) {
                cache.store(key, &Payload::Symbolic(Box::new(sum.clone())), None);
            }
            found
        });
    let identity = match refs::closed_form_identity(&sum, written.as_ref()) {
        Ok(identity) => identity,
        Err(_) if var == sva_formula::Var::T => refs::identity(&held.tys, id)?,
        Err(e) => return Err(e),
    };
    let score = held.alias_score(id);
    let extent = held.extents.of(id);
    let samples = extent.len();
    let key = crate::cache::buffer_key(
        identity,
        held.config.rate,
        extent,
        held.tys.ty(id).width as usize,
        score,
    );
    if let Ok(sum) = &sum {
        held.symbolic.insert(id, sum.clone());
    }
    // FORMAT 9.3: the label belongs to the value, so an entry without one is no value.
    if let Some((hit, label)) = warm(held, id, key, samples, cache) {
        held.buffers.insert(id, hit);
        held.labels.insert(id, label);
        return Ok(());
    }
    let (buffer, label) = match (&sum, &written) {
        (Err(_), None) => pointwise::point_sample(held, id, score)?,
        _ => sampled_form(held, extent, &sum, written.as_ref(), score)
            .map_err(|e| collapse_refused(held, id, &e))?,
    };
    store(key, &buffer, &label, cache);
    held.buffers.insert(id, buffer);
    held.labels.insert(id, label);
    Ok(())
}

fn collapse_refused(held: &Render, id: NodeId, e: &sva_samples::CollapseError) -> EngineError {
    EngineError::refused(Diagnostic {
        code: e.code().to_string(),
        message: e.to_string(),
        location: Located::at(held.tys.name(id), None),
        help: e.help().to_string(),
    })
}

/// A closed form with a spectral sum takes the six rows over it; one without takes the point-sampled
/// row over the written closed form itself, which is what FORMAT 9.1's no-dual row is.
fn sampled_form(
    held: &Render,
    extent: Extent,
    sum: &Result<SpectralSum, EngineError>,
    written: Option<&sva_formula::ClosedForm>,
    score: AliasScore,
) -> Result<(Buffer, Label), sva_samples::CollapseError> {
    let (rate, profile) = (held.config.rate, &held.config.profile);
    match (sum, written) {
        (Ok(sum), written) => {
            sva_samples::of_spectral_sum_or_point(sum, written, rate, extent, profile, score)
        }
        (Err(_), Some(form)) => sva_samples::render(form, rate, extent, profile, score),
        (Err(_), None) => unreachable!("a closed form with neither view is point-sampled above"),
    }
}

/// A spectral sum is rate-free, so one entry answers every rate the same closed form is read at.
fn remembered(
    held: &Render,
    id: NodeId,
    key: Option<sva_formula::Hash>,
    cache: Option<&Lens>,
) -> Option<SpectralSum> {
    let entry = cache?.load(key?, held.tys.name(id), Expected::Symbolic)?;
    entry.payload.symbolic().cloned()
}

/// A warm entry is the same bytes the cold run would write, so a hit is returned as it
/// stands rather than checked against a second collapse.
fn warm(
    held: &Render,
    id: NodeId,
    key: sva_formula::Hash,
    samples: usize,
    cache: Option<&Lens>,
) -> Option<(Buffer, Label)> {
    let expected = Expected::Samples {
        rate: held.config.rate,
        width: held.tys.ty(id).width as usize,
        samples,
    };
    let entry = cache?.load(key, held.tys.name(id), expected)?;
    Some((entry.payload.samples().cloned()?, entry.label?))
}

fn store(key: sva_formula::Hash, buffer: &Buffer, label: &Label, cache: Option<&Lens>) {
    if let Some(cache) = cache {
        cache.store(
            key,
            &Payload::Samples(Box::new(buffer.clone())),
            Some(label),
        );
    }
}

fn frames_of(held: &mut Render, id: NodeId, cache: Option<&Lens>) -> Result<(), EngineError> {
    let Value::Cast(Cast::Stft { window, hop }, source) = *held.tys.value(id) else {
        return Err(not_frames(held, id));
    };
    let buffer = held
        .buffers
        .get(&source)
        .ok_or_else(|| not_frames(held, id))?;
    let key = frames_key(
        crate::cache::buffer_key(
            refs::identity(&held.tys, source)?,
            buffer.rate,
            buffer.extent(),
            buffer.width,
            AliasScore::NotAsked,
        ),
        window,
        hop,
    );
    if let Some(entry) = cache.and_then(|c| c.load(key, held.tys.name(id), Expected::Frames))
        && let Payload::Frames(frames) = entry.payload
    {
        held.frames.insert(id, *frames);
        return Ok(());
    }
    let frames = stft::forward(buffer, window, hop).map_err(|e| sampled::refused(held, id, &e))?;
    if let Some(cache) = cache {
        cache.store(key, &Payload::Frames(Box::new(frames.clone())), None);
    }
    held.frames.insert(id, frames);
    Ok(())
}

fn not_frames(held: &Render, id: NodeId) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "cast.stft_needs_samples".to_string(),
        message: format!("`{}` holds no frames to read", held.tys.name(id)),
        location: Located::at(held.tys.name(id), None),
        help: "write stft(sample(x), window=, hop=)".to_string(),
    })
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
