// Concern: holds a composition's decided types, the buffers a reading needed and each label | Non-concern: ordering the work (schedule.rs), a sampled shader (sampled.rs) | IO: (&Graph, target) -> Render

mod answer;
mod pointwise;
mod sampled;
mod slots;

use std::collections::BTreeMap;

use sva_ast::Graph;
use sva_formula::{Held, NodeId, SpectralSum, hash_closed_form};
use sva_samples::{
    AliasScore, Buffer, FilterTrace, Frames, Horizon, Label, PSYCHOACOUSTIC_V1, Profile, stft,
};

use crate::bindings::Binding;
use crate::cache::{Cost, Expected, Payload, PayloadKind, frames_key, symbolic_key};
use crate::cast::Cast;
use crate::error::{Diagnostic, EngineError, Located};
use crate::instantiate;
use crate::query::Ask;
use crate::refs;
use crate::schedule::{self, Schedule};
use crate::typing::{self, Typing, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct RenderConfig {
    pub rate: u32,
    pub horizon: Horizon,
    pub profile: Profile,
    /// What the caller means to read. An empty list is audio out, which collapses the root.
    pub asks: Vec<Ask>,
    /// The operation count this render may pay; the profile's own until a caller raises it.
    pub flop_budget: u128,
}

impl RenderConfig {
    pub fn seconds(rate: u32, secs: f64) -> RenderConfig {
        RenderConfig {
            rate,
            horizon: Horizon::secs(0.0, secs),
            profile: PSYCHOACOUSTIC_V1,
            asks: Vec::new(),
            flop_budget: PSYCHOACOUSTIC_V1.flop_budget,
        }
    }

    pub fn asking(mut self, asks: Vec<Ask>) -> RenderConfig {
        self.asks = asks;
        self
    }
}

pub use answer::{answer, answer_buffer, sketch_atom};

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
}

impl Render {
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
    cache: Option<&dyn crate::cache::Cache>,
) -> Result<Render, EngineError> {
    let instances = instantiate::instantiate(graph, target)?;
    let held = instances.instance_of(target)?;
    let order = schedule::schedule_from(&instances, std::slice::from_ref(&held))?;
    let tys = typing::infer_all(&instances, &order)?;
    let root = tys
        .id(&held)
        .ok_or_else(|| EngineError::UnknownNode(held.clone()))?;
    let schedule = schedule::plan(&tys, &order, root, &config.asks);

    let bindings = tys
        .paths()
        .filter_map(|(path, id)| Some((id, resolved(&instances, path)?)))
        .collect();
    let mut held = Render {
        root,
        tys,
        buffers: BTreeMap::new(),
        frames: BTreeMap::new(),
        symbolic: BTreeMap::new(),
        labels: BTreeMap::new(),
        traces: Vec::new(),
        config,
        schedule,
        bindings,
    };
    affordable(&held)?;
    for id in held.schedule.materialize.clone() {
        materialize(&mut held, id, cache)?;
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
        help: format!("pass --flop-budget {} to render it anyway", counted.total),
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
/// `--as` answers off that one form rather than walking the graph again. A form that
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

fn materialize(
    held: &mut Render,
    id: NodeId,
    cache: Option<&dyn crate::cache::Cache>,
) -> Result<(), EngineError> {
    match held.tys.ty(id).held {
        Held::Frames => frames_of(held, id, cache),
        Held::Sampled => sampled::run(held, id, cache),
        _ => collapse_closed_form(held, id, cache),
    }
}

/// The one place a closed form becomes samples: `sample(...)` and the render root, and nothing else.
fn collapse_closed_form(
    held: &mut Render,
    id: NodeId,
    cache: Option<&dyn crate::cache::Cache>,
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
            let began = Cost::begun();
            let found = refs::spectral_sum_of(&held.tys, id, var);
            if let (Ok(sum), Some(key), Some(cache)) = (&found, symbolic, cache) {
                let payload = Payload::Symbolic(Box::new(sum.clone()));
                if cache.worth_storing(began.elapsed(), payload.bytes(), PayloadKind::Symbolic) {
                    cache.store(key, &payload, &[], None);
                }
            }
            found
        });
    let identity = match refs::closed_form_identity(&sum, written.as_ref()) {
        Ok(identity) => identity,
        Err(_) if var == sva_formula::Var::T => refs::identity(&held.tys, id)?,
        Err(e) => return Err(e),
    };
    let score = held.alias_score(id);
    let key = crate::cache::buffer_key(
        identity,
        held.config.rate,
        held.config.horizon.start_secs,
        held.tys.ty(id).width as usize,
        score,
    );
    if let Ok(sum) = &sum {
        held.symbolic.insert(id, sum.clone());
    }
    // FORMAT 9.3: the label belongs to the value, so an entry without one is no value.
    if let Some((hit, label)) = warm(held, id, key, cache) {
        held.buffers.insert(id, hit);
        held.labels.insert(id, label);
        return Ok(());
    }
    let began = Cost::begun();
    let (buffer, label) = match (&sum, &written) {
        (Err(_), None) => pointwise::point_sample(held, id, score)?,
        _ => sampled_form(held, &sum, written.as_ref(), score).map_err(|e| {
            EngineError::refused(Diagnostic {
                code: e.code().to_string(),
                message: e.to_string(),
                location: Located::at(held.tys.name(id), None),
                help: e.help().to_string(),
            })
        })?,
    };
    store(key, &buffer, &label, began.elapsed(), cache);
    held.buffers.insert(id, buffer);
    held.labels.insert(id, label);
    Ok(())
}

/// A closed form with a spectral sum takes the six rows over it; one without takes the point-sampled
/// row over the written closed form itself, which is what FORMAT 9.1's no-dual row is.
fn sampled_form(
    held: &Render,
    sum: &Result<SpectralSum, EngineError>,
    written: Option<&sva_formula::ClosedForm>,
    score: AliasScore,
) -> Result<(Buffer, Label), sva_samples::CollapseError> {
    let (rate, horizon, profile) = (held.config.rate, held.config.horizon, &held.config.profile);
    match (sum, written) {
        (Ok(sum), written) => {
            sva_samples::of_spectral_sum_or_point(sum, written, rate, horizon, profile, score)
        }
        (Err(_), Some(form)) => sva_samples::render(form, rate, horizon, profile, score),
        (Err(_), None) => unreachable!("a closed form with neither view is point-sampled above"),
    }
}

/// A spectral sum is rate-free, so one entry answers every rate the same closed form is read at.
fn remembered(
    held: &Render,
    id: NodeId,
    key: Option<sva_formula::Hash>,
    cache: Option<&dyn crate::cache::Cache>,
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
    cache: Option<&dyn crate::cache::Cache>,
) -> Option<(Buffer, Label)> {
    let expected = Expected::Samples {
        rate: held.config.rate,
        width: held.tys.ty(id).width as usize,
        samples: held.config.horizon.len(held.config.rate).ok()?,
    };
    let entry = cache?.load(key, held.tys.name(id), expected)?;
    Some((entry.payload.samples().cloned()?, entry.label?))
}

fn store(
    key: sva_formula::Hash,
    buffer: &Buffer,
    label: &Label,
    cost: std::time::Duration,
    cache: Option<&dyn crate::cache::Cache>,
) {
    let Some(cache) = cache else { return };
    let payload = Payload::Samples(Box::new(buffer.clone()));
    if cache.worth_storing(cost, payload.bytes(), PayloadKind::Samples) {
        cache.store(key, &payload, &[], Some(label));
    }
}

fn frames_of(
    held: &mut Render,
    id: NodeId,
    cache: Option<&dyn crate::cache::Cache>,
) -> Result<(), EngineError> {
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
            buffer.origin_secs,
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
    let began = Cost::begun();
    let frames = stft::forward(buffer, window, hop).map_err(|e| sampled::refused(held, id, &e))?;
    if let Some(cache) = cache {
        let payload = Payload::Frames(Box::new(frames.clone()));
        if cache.worth_storing(began.elapsed(), payload.bytes(), PayloadKind::Frames) {
            cache.store(key, &payload, &[], None);
        }
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

/// One instance's resolved parameters, as the composer wrote them at the call site.
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
