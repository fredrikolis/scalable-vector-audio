// Concern: renders a target until every later sample is provably under half an LSB | Non-concern: bounding one node class (envelope.rs) | IO: (&Graph, target, bits, max) -> a Render ending at silence

mod envelope;
mod floor;
mod range;
mod ringing;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use sva_ast::Graph;
use sva_formula::{Hash, NodeId};
use sva_samples::{Buffer, Horizon, Label};

pub(crate) use envelope::Live;
use envelope::{Bounds, Envelope, Forms, Grid, STEP, Unbounded};

use super::{Prepared, Render, RenderConfig, materialize, prepared, run};
use crate::cache::{Cache, Cost, Expected, Payload, Recording, Slots};
use crate::error::{Diagnostic, EngineError, Located};
use crate::schedule::{Schedule, dependencies_first};

/// Silence at `bits` is every later sample under `2^-bits` of full scale, proven by `max_secs`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Silent {
    pub bits: u32,
    pub max_secs: f64,
}

impl Silent {
    pub fn threshold(self) -> f64 {
        2f64.powi(-(self.bits as i32))
    }
}

/// The horizon ends at the last sample at or over the threshold. The bound proves every sample
/// from its grid instant on is under it, and the render between proves the rest.
pub fn render_until_silent(
    graph: &Graph,
    target: &str,
    config: RenderConfig,
    silent: Silent,
    cache: Option<&dyn Cache>,
    slots: Option<&Slots>,
) -> Result<Render, EngineError> {
    let held = prepared(graph, target)?;
    let rate = config.rate;
    let width = held.tys.ty(held.root).width as usize;
    let key = silent_key(held.identity(&config.asks)?, &config, silent);
    // A volatile root is kept in its slot by the render itself, and nowhere else.
    let volatile = super::volatile::mark(&held.instances, &held.tys, &config, &held.target)?;
    let recording = cache
        .filter(|_| volatile.slot(held.root).is_none())
        .map(|c| Recording::over(c, slots));
    let lens = recording.as_ref().map(|r| r.at(None));
    let store = lens.as_ref().map(|l| l as &dyn Cache);
    if let Some((buffer, label)) = recalled(store, key, &held, rate, width) {
        let config = ended(config, buffer.len());
        let render = run(held, config, cache, slots, Some((buffer, label)))?;
        return Ok(counted(render, recording));
    }
    let began = Cost::begun();
    let (proven, proofs) = proven_at(&held, &config, silent)?;
    let mut render = run(held, ended(config, proven.max(1)), cache, slots, None)?;
    render.proofs = proofs;
    let heard = render.buffers.get(&render.root).map_or(0, |b| {
        (0..b.len())
            .rev()
            .find(|&n| (0..b.width).any(|c| b.plane(c)[n].abs() >= silent.threshold()))
            .map_or(0, |n| n + 1)
    });
    trimmed(&mut render, heard.max(1));
    if let (Some(store), Some(buffer), Some(label)) = (
        store,
        render.buffers.get(&render.root),
        render.labels.get(&render.root),
    ) {
        remember(store, key, buffer, label, began.elapsed());
    }
    Ok(counted(render, recording))
}

/// The silent entry's own lookups, before every lookup the render made.
fn counted(mut render: Render, recording: Option<Recording>) -> Render {
    let Some(recording) = recording else {
        return render;
    };
    let mut stats = recording.finish();
    if let Some(made) = render.cache_stats.take() {
        stats.lookups.extend(made.lookups);
    }
    render.cache_stats = Some(stats);
    render
}

/// The first sample from which the root's bound stays under the threshold, and the proofs
/// over the whole grid it took.
pub(super) fn proven_at(
    held: &Prepared,
    config: &RenderConfig,
    silent: Silent,
) -> Result<(usize, u64), EngineError> {
    let rate = f64::from(config.rate);
    let start = config.horizon.start_secs;
    let samples = ((silent.max_secs - start) * rate).ceil().max(0.0) as usize;
    let grid = Grid {
        start,
        rate,
        points: samples / STEP + 1,
    };
    let rendered = |id: NodeId, end: f64| heard_alone(&held.tys, id, config, end);
    let threshold = silent.threshold();
    let mut level = threshold;
    let forms = Forms::new(Cow::Borrowed(&held.tys), Cow::Borrowed(config));
    let mut proofs = 0;
    loop {
        proofs += 1;
        let mut bounds = Bounds::new(&forms, grid.clone(), &rendered, level);
        let envelope = bounded(&held.tys, held.root, &mut bounds, silent)?;
        if let Some(j) = envelope.at.iter().position(|v| *v < threshold) {
            return Ok((j * STEP, proofs));
        }
        let last = envelope.at.last().copied().unwrap_or(f64::INFINITY);
        if !bounds.held_flat || !last.is_finite() {
            return Err(not_by(held, &envelope, silent));
        }
        // Each round at least halves the level, so a held bound is stepped past, or none holds.
        level *= threshold / last / 2.0;
    }
}

/// What no block changes, kept across a stream's proofs under the typing and config it holds:
/// each node rendered alone over a crop's window, and each closed form compiled.
pub(crate) struct Kept {
    heard: RefCell<BTreeMap<(NodeId, u64), Buffer>>,
    forms: Forms<'static>,
}

impl Kept {
    pub(crate) fn new(tys: crate::typing::Typing, config: RenderConfig) -> Kept {
        Kept {
            heard: RefCell::default(),
            forms: Forms::new(Cow::Owned(tys), Cow::Owned(config)),
        }
    }
}

/// A bound on every sample of `root` from `now` on, from the states `live` holds there.
pub(crate) fn bound_from(
    kept: &Kept,
    root: NodeId,
    silent: Silent,
    live: &dyn Live,
    now: usize,
) -> Result<f64, EngineError> {
    let (tys, config) = (&*kept.forms.tys, &*kept.forms.config);
    let heard = &kept.heard;
    let grid = Grid {
        start: now as f64 / f64::from(config.rate),
        rate: f64::from(config.rate),
        points: 1,
    };
    let rendered = |id: NodeId, end: f64| {
        if let Some(buffer) = heard.borrow().get(&(id, end.to_bits())) {
            return Ok(buffer.clone());
        }
        let buffer = heard_alone(tys, id, config, end)?;
        heard
            .borrow_mut()
            .insert((id, end.to_bits()), buffer.clone());
        Ok(buffer)
    };
    let mut bounds = Bounds::new(&kept.forms, grid, &rendered, silent.threshold());
    bounds.live = Some(live);
    Ok(bounded(tys, root, &mut bounds, silent)?.at[0])
}

/// The root's envelope, or the refusal no bound or a level held forever makes.
fn bounded(
    tys: &crate::typing::Typing,
    root: NodeId,
    bounds: &mut Bounds,
    silent: Silent,
) -> Result<Envelope, EngineError> {
    let envelope = match bounds.of(root)? {
        Ok(envelope) => envelope,
        Err(Unbounded { node, class }) => {
            return Err(refusal(
                tys,
                root,
                "engine.no_tail_bound",
                format!(
                    "`{node}` is {class}, and no bound on its tail is derived yet, so silence \
                     is never proven for it."
                ),
                "render it to a stated --to instead, or crop it to a window",
            ));
        }
    };
    let threshold = silent.threshold();
    if envelope.floor >= threshold {
        return Err(refusal(
            tys,
            root,
            "engine.never_silent",
            format!(
                "it returns to {} forever, at or above the {}-bit floor of {}.",
                dbfs(envelope.floor),
                silent.bits,
                dbfs(threshold)
            ),
            "crop it, or give it a release",
        ));
    }
    Ok(envelope)
}

fn not_by(held: &Prepared, envelope: &Envelope, silent: Silent) -> EngineError {
    let last = envelope.at.last().copied().unwrap_or(f64::INFINITY);
    not_silent_by(&held.tys, held.root, Some(last), silent)
}

/// `last` is the bound the latest proof found, `None` where none has run.
pub(crate) fn not_silent_by(
    tys: &crate::typing::Typing,
    root: NodeId,
    last: Option<f64>,
    silent: Silent,
) -> EngineError {
    let floor = format!(
        "the {}-bit floor of {}",
        silent.bits,
        dbfs(silent.threshold())
    );
    let found = match last {
        Some(last) => format!(
            "its bound at {}s is {}, not under {floor}",
            silent.max_secs,
            dbfs(last)
        ),
        None => format!(
            "no block has ended by {}s to prove it under {floor}",
            silent.max_secs
        ),
    };
    refusal(
        tys,
        root,
        "engine.not_silent_by",
        format!("{found}, so silence is not proven by then."),
        "if it decays, raise --max or lower the bits",
    )
}

fn dbfs(v: f64) -> String {
    match v.is_finite() {
        true => format!("{:.1} dBFS", 20.0 * v.log10()),
        false => "unbounded".to_string(),
    }
}

fn refusal(
    tys: &crate::typing::Typing,
    root: NodeId,
    code: &str,
    message: String,
    help: &str,
) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(tys.name(root), None),
        help: help.to_string(),
    })
}

fn ended(mut config: RenderConfig, samples: usize) -> RenderConfig {
    let start = config.horizon.start_secs;
    config.horizon = Horizon::secs(start, start + samples as f64 / f64::from(config.rate));
    config
}

/// Every buffer this render holds, cut to the root's silence; each is causal from the
/// window's start, so the cut is the value a shorter render of it would have written.
fn trimmed(render: &mut Render, samples: usize) {
    for buffer in render.buffers.values_mut() {
        if buffer.len() > samples {
            let origin = buffer.origin_secs;
            *buffer = Buffer::of_planes(
                buffer.rate,
                (0..buffer.width)
                    .map(|c| buffer.plane(c)[..samples].to_vec())
                    .collect(),
            );
            buffer.origin_secs = origin;
        }
    }
    render.config = ended(render.config.clone(), samples);
}

/// One node on its own, over the window a crop closes: nothing is kept past this bound.
fn heard_alone(
    tys: &crate::typing::Typing,
    id: NodeId,
    config: &RenderConfig,
    end: f64,
) -> Result<Buffer, EngineError> {
    let mut held = Render {
        root: id,
        tys: tys.clone(),
        buffers: BTreeMap::new(),
        frames: BTreeMap::new(),
        symbolic: BTreeMap::new(),
        labels: BTreeMap::new(),
        traces: Vec::new(),
        config: RenderConfig {
            horizon: Horizon::secs(config.horizon.start_secs, end),
            asks: Vec::new(),
            ..config.clone()
        },
        schedule: Schedule {
            materialize: Vec::new(),
            symbolic: Vec::new(),
            compose: Vec::new(),
        },
        bindings: BTreeMap::new(),
        cache_stats: None,
        proofs: 0,
    };
    for member in dependencies_first(tys, id, &mut BTreeSet::new()) {
        materialize(&mut held, member, None)?;
    }
    held.buffers
        .remove(&id)
        .ok_or_else(|| EngineError::UnknownNode(tys.name(id).to_string()))
}

const SILENT_TAG: u64 = 0x73_69_6c_65_6e_74_00_01;

/// The root at one rate and origin, silent at `bits` by `max`, scored or not: the length is
/// the value's own.
fn silent_key((identity, scored): (Hash, bool), config: &RenderConfig, silent: Silent) -> Hash {
    crate::cache::mixed(
        identity,
        &[
            u64::from(config.rate),
            config.horizon.start_secs.to_bits(),
            u64::from(silent.bits),
            silent.max_secs.to_bits(),
            u64::from(scored),
            SILENT_TAG,
        ],
    )
}

/// The length is stored as a one-sample record under the silent key, and the samples under
/// that key and the length together.
fn samples_key(key: Hash, samples: usize) -> Hash {
    crate::cache::mixed(key, &[samples as u64, SILENT_TAG])
}

fn recalled(
    cache: Option<&dyn Cache>,
    key: Hash,
    held: &Prepared,
    rate: u32,
    width: usize,
) -> Option<(Buffer, Label)> {
    let cache = cache?;
    let name = held.tys.name(held.root);
    let length = cache.load(
        key,
        name,
        Expected::Samples {
            rate,
            width: 1,
            samples: 1,
        },
    )?;
    let samples = length.payload.samples()?.plane(0)[0] as usize;
    let entry = cache.load(
        samples_key(key, samples),
        name,
        Expected::Samples {
            rate,
            width,
            samples,
        },
    )?;
    Some((entry.payload.samples().cloned()?, entry.label?))
}

fn remember(
    cache: &dyn Cache,
    key: Hash,
    buffer: &Buffer,
    label: &Label,
    cost: std::time::Duration,
) {
    let payload = Payload::Samples(Box::new(buffer.clone()));
    if !cache.worth_storing(cost, payload.bytes(), crate::cache::PayloadKind::Samples) {
        return;
    }
    cache.store(samples_key(key, buffer.len()), &payload, &[], Some(label));
    let length = Buffer::of_planes(buffer.rate, vec![vec![buffer.len() as f64]]);
    cache.store(key, &Payload::Samples(Box::new(length)), &[], None);
}
