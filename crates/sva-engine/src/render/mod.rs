// Concern: holds a composition's decided types, the buffers a reading needed and each label | Non-concern: ordering the work (schedule.rs), a sampled shader (sampled.rs) | IO: (&Graph, target) -> Render

mod answer;
pub(crate) mod bound;
mod drive;
pub(crate) mod extent;
mod pointwise;
mod pull;
mod quiet;
mod reach;
mod sampled;
mod slots;
mod stream;
mod terms;
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
/// where its support ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    pub rate: u32,
    pub range: Range,
    /// Where the render stops before the range ends; `None` reads the range to its end.
    pub until: Option<Until>,
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
pub use drive::Block;
pub use quiet::{QUIET_AFTER_SECS, QUIET_LEVEL, QuietTail, quiet_tails};
pub use stream::{STREAMED, Stream, StreamConfig};
pub use terms::{Handle, NOTES};
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
    /// The samples the root was read over; `None` where no reading needed any.
    pub range: Option<Extent>,
    pub(crate) unranged: Option<EngineError>,
    pub(crate) extents: extent::Extents,
    identities: std::cell::RefCell<BTreeMap<NodeId, sva_formula::Hash>>,
    prefixes: std::cell::RefCell<refs::Prefixes>,
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
            range: None,
            unranged: None,
            extents: extent::Extents::default(),
            identities: Default::default(),
            prefixes: Default::default(),
        }
    }

    /// A node's content address, each node under it named once however often it is asked.
    pub(crate) fn identity(&self, id: NodeId) -> Result<sva_formula::Hash, EngineError> {
        refs::identity_in(&self.tys, id, &mut self.identities.borrow_mut())
    }

    /// The node's switches and its identity before each, asked once however often.
    #[cfg_attr(not(test), expect(dead_code, reason = "segment keys read it"))]
    pub(crate) fn prefixes<T>(&self, ask: impl FnOnce(&mut refs::Walk) -> T) -> T {
        let (mut held, mut named) = (self.prefixes.borrow_mut(), self.identities.borrow_mut());
        ask(&mut refs::Walk::new(
            &self.tys,
            self.config.rate,
            &mut held,
            &mut named,
        ))
    }

    /// `key` with what a node's samples read beyond its content: the precision a collapse
    /// truncates at.
    pub(crate) fn keyed(&self, key: sva_formula::Hash) -> sva_formula::Hash {
        crate::cache::precise_key(key, self.config.profile.precision_bits)
    }

    pub fn work(&self) -> crate::flops::Work {
        crate::flops::Work {
            samples: self.range.map_or(0, |range| range.len() as u64),
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

pub(crate) fn run(
    prepared: Prepared,
    config: RenderConfig,
    recording: Option<&Recording>,
) -> Result<Render, EngineError> {
    let (mut held, instances, target, looped, costed) = planned(prepared, config)?;
    let lenses = Lenses {
        volatile: volatile::mark(&instances, &held, &target)?,
        ..Lenses::of(recording, &held)
    };
    pull::computed(&mut held, &lenses, &looped, &costed)?;
    compose_read(&mut held);
    stamp(&mut held);
    Ok(held)
}

type Planned<'g> = (
    Render,
    instantiate::Instances<'g>,
    String,
    BTreeMap<String, String>,
    Vec<NodeId>,
);

fn planned(prepared: Prepared<'_>, config: RenderConfig) -> Result<Planned<'_>, EngineError> {
    sampled::on_the_grid(&prepared.tys, config.rate)?;
    let Prepared {
        instances,
        order,
        tys,
        root,
        target,
    } = prepared;
    let schedule = schedule::plan(&tys, &order, root, &config.asks);
    let audio = schedule::plan(&tys, &order, root, &[]).materialize;
    let costed = match schedule.materialize.is_empty() && counts(&config.asks) {
        true => audio.clone(),
        false => schedule.materialize.clone(),
    };
    let looped = order.looped();
    let bindings = tys
        .paths()
        .filter_map(|(path, id)| Some((id, resolved(&instances, path)?)))
        .collect();
    let mut held = Render::shell(tys, root, config, schedule);
    held.bindings = bindings;
    reach::ranged(&mut held, &costed)?;
    Ok((held, instances, target, looped, costed))
}

/// The range and every extent a render of `target` decides, and no sample.
pub fn plan(graph: &Graph, target: &str, config: RenderConfig) -> Result<Render, EngineError> {
    planned(prepared(graph, target)?, config).map(|(held, ..)| held)
}

/// A `flops` reading counts what the audio render would run, over the extents it would.
fn counts(asks: &[Ask]) -> bool {
    asks.iter()
        .any(|ask| ask.representation == crate::query::Representation::Flops)
}

/// Nothing steps or substitutes a loop of refs, so a node held over one refuses.
pub(super) fn unlooped(
    held: &Render,
    needed: &std::collections::BTreeSet<NodeId>,
    looped: &BTreeMap<String, String>,
) -> Result<(), EngineError> {
    for id in held
        .schedule
        .materialize
        .iter()
        .filter(|id| needed.contains(id))
    {
        if let Some(member) = looped.get(held.tys.name(*id)) {
            let at = held.tys.id(member).expect("a loop's own path types");
            return Err(refs::cyclic(&held.tys, at));
        }
    }
    Ok(())
}

/// A reading that materializes nothing is never refused for cost: asking what a render
/// would cost is not paying for it. Anything that runs is counted first, and a count past
/// the budget refuses before a sample is computed.
pub(super) fn affordable(held: &Render) -> Result<(), EngineError> {
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
            "this render counts {} operations, over the budget of {}; `{}` dominates it at {} \
             by {}",
            counted.total, counted.budget, over.node, over.subtree, over.route
        ),
        location: Located::at(held.tys.name(held.root), None),
        help: format!("pass --flop-budget {total} to render it anyway"),
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
    pub(super) recording: Option<&'r Recording>,
    volatile: volatile::Volatile,
    forks: std::collections::BTreeSet<NodeId>,
    root: NodeId,
    /// The machines one run in the store answers wherever they are read.
    pub(super) runs: std::collections::BTreeSet<NodeId>,
}

impl<'r> Lenses<'r> {
    /// No parameter is volatile.
    pub(super) fn of(recording: Option<&'r Recording>, held: &Render) -> Lenses<'r> {
        let order = &held.schedule.materialize;
        Lenses {
            recording,
            volatile: volatile::Volatile::default(),
            forks: schedule::forks(&held.tys, order),
            root: held.root,
            runs: drive::node::runnable(held, order),
        }
    }

    /// The same view through another recording.
    pub(super) fn with<'s>(&self, recording: Option<&'s Recording>) -> Lenses<'s> {
        Lenses {
            recording,
            volatile: volatile::Volatile::default(),
            forks: self.forks.clone(),
            root: self.root,
            runs: self.runs.clone(),
        }
    }
}

impl Lenses<'_> {
    pub(super) fn slot(&self, id: NodeId) -> Option<sva_formula::Hash> {
        self.volatile.slot(id)
    }

    pub(super) fn at(&self, id: NodeId) -> Option<Lens<'_>> {
        let fork = self.forks.contains(&id);
        self.recording
            .map(|r| r.at(self.volatile.slot(id), fork, id == self.root))
    }

    /// What a reading asks for, what the policy keeps, and what those read that the store
    /// does not answer. A ledger holds them all.
    pub(super) fn needed(&self, held: &Render) -> std::collections::BTreeSet<NodeId> {
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

    pub(super) fn answered(&self, held: &Render, id: NodeId) -> bool {
        let (Some(recording), Held::Sampled) = (self.recording, held.tys.ty(id).held) else {
            return false;
        };
        if !self.runs.contains(&id) {
            return sampled::key(held, id).is_ok_and(|(key, _)| recording.holds(key));
        }
        let extent = held.extents.of(id);
        drive::node::run_key_of(held, id).is_ok_and(|key| {
            recording
                .run_span(key)
                .is_some_and(|span| span.start <= extent.start && extent.end <= span.end)
        })
    }
}

/// The operations a sampled node's program runs over `extent`, where one lowers it; with
/// no extent, the whole program's for one sample.
pub(crate) fn sampled_ops(held: &Render, id: NodeId, extent: Option<Extent>) -> Option<u128> {
    let program = sampled::program(held, id).ok()?;
    match extent {
        Some(extent) => program.priced(held, extent),
        None => program
            .renderer
            .ops(&program.layout)
            .ok()
            .map(|ops| ops as u128),
    }
}

/// What the schedule orders before `id`, and every buffer its program reads behind those.
pub(super) fn reads(held: &Render, id: NodeId) -> Vec<NodeId> {
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
pub(super) fn materialize(
    held: &mut Render,
    id: NodeId,
    lenses: &Lenses,
) -> Result<(), EngineError> {
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
        let key = match lens {
            Some(_) => Some(sampled::key(held, id)?),
            None => None,
        };
        if let Some((key, samples)) = key
            && let Some((hit, label)) = warm(held, id, key, samples, lens.as_ref())
        {
            held.buffers.insert(id, hit);
            held.labels.insert(id, label);
            return Ok(());
        }
        for read in reads(held, id) {
            materialize(held, read, lenses)?;
        }
        return sampled::run(held, id, key.map(|(key, _)| key), lens.as_ref());
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
    if let (Err(e), None) = (&sum, &written)
        && var != sva_formula::Var::T
    {
        return Err(e.clone());
    }
    let identity = match cache {
        Some(_) => Some(
            refs::closed_form_identity(&sum, written.as_ref()).or_else(|_| held.identity(id))?,
        ),
        None => None,
    };
    let score = held.alias_score(id);
    let (rate, profile) = (held.config.rate, &held.config.profile);
    let asked = held.extents.of(id);
    let planned = match (&sum, &written) {
        (Err(_), None) => None,
        _ => Some(
            sva_samples::planned(sum.as_ref().ok(), written.as_ref(), rate, asked, profile)
                .map_err(|e| collapse_refused(held, id, &e))?,
        ),
    };
    // A run over where the row writes nonzero is the same bits there, so the key names that.
    let (over, route) = match (&planned, score) {
        (Some(planned), AliasScore::NotAsked) => (planned.nonzero(rate, asked), planned.route()),
        _ => (asked, Vec::new()),
    };
    let width = held.tys.ty(id).width as usize;
    let key = match identity {
        Some(identity) => Some(held.keyed(crate::cache::mixed(
            crate::cache::buffer_key(identity, rate, over, width, score),
            &route,
        ))),
        None => None,
    };
    if let Ok(sum) = &sum {
        held.symbolic.insert(id, sum.clone());
    }
    // FORMAT 9.3: the label belongs to the value, so an entry without one is no value.
    if let Some((hit, label)) = key.and_then(|key| warm(held, id, key, over.len(), cache)) {
        held.buffers.insert(id, padded(hit, asked));
        held.labels.insert(id, label);
        return Ok(());
    }
    let (buffer, label) = match planned {
        None => pointwise::point_sample(held, id, score)?,
        Some(planned) => planned
            .run(rate, over, profile, score)
            .map_err(|e| collapse_refused(held, id, &e))?,
    };
    if let Some(key) = key {
        store(key, &buffer, &label, cache);
    }
    held.buffers.insert(id, padded(buffer, asked));
    held.labels.insert(id, label);
    Ok(())
}

/// A buffer over part of `asked`, zero over the rest.
fn padded(buffer: Buffer, asked: Extent) -> Buffer {
    if buffer.extent() == asked {
        return buffer;
    }
    let at = (buffer.start - asked.start).max(0) as usize;
    let planes = buffer
        .planes
        .iter()
        .map(|plane| {
            let mut out = vec![0.0; asked.len()];
            if !plane.is_empty() {
                out[at..at + plane.len()].copy_from_slice(plane);
            }
            out
        })
        .collect();
    let mut out = Buffer::of_planes(buffer.rate, planes);
    out.start = asked.start;
    out
}

fn collapse_refused(held: &Render, id: NodeId, e: &sva_samples::CollapseError) -> EngineError {
    EngineError::refused(Diagnostic {
        code: e.code().to_string(),
        message: e.to_string(),
        location: Located::at(held.tys.name(id), None),
        help: e.help().to_string(),
    })
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
pub(super) fn warm(
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

pub(super) fn store(key: sva_formula::Hash, buffer: &Buffer, label: &Label, cache: Option<&Lens>) {
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
    let key = match cache {
        Some(_) => Some(frames_key(
            held.keyed(crate::cache::buffer_key(
                held.identity(source)?,
                buffer.rate,
                buffer.extent(),
                buffer.width,
                AliasScore::NotAsked,
            )),
            window,
            hop,
        )),
        None => None,
    };
    if let (Some(cache), Some(key)) = (cache, key)
        && let Some(entry) = cache.load(key, held.tys.name(id), Expected::Frames)
        && let Payload::Frames(frames) = entry.payload
    {
        held.frames.insert(id, *frames);
        return Ok(());
    }
    let frames = stft::forward(buffer, window, hop).map_err(|e| sampled::refused(held, id, &e))?;
    if let (Some(cache), Some(key)) = (cache, key) {
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
