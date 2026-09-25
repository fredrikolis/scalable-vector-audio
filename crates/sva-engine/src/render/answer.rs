// Concern: takes one reading off the representation a node declares, a ledger edge by edge | Non-concern: naming the observations (query.rs), the arithmetic of one | IO: (&Render, node) -> Answer

use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};
use sva_formula::{AUDIBLE_CEILING_HZ, Line, SpectralSum, Var, d_dt, envelope, line_atoms};
use sva_samples::{
    AliasScore, Buffer, Consumes, Horizon, Peak, PitchFrame, Source, measure::bands,
    measure::crest, measure::envelope, measure::formants, measure::loudness, measure::pitch,
    measure::spectrum, measure::stereo, measure_alias,
};

use crate::error::{Diagnostic, EngineError, Located};
use crate::query::{Answer, DEFAULT_FRAME_SECS, Output, Representation};
use crate::refs;
use crate::render::Render;
use crate::typing::Value;

/// A closed form answers off its spectral sum and a buffer off its samples; its own `Ty`
/// decides which, never a flag on the observation.
pub fn answer(
    render: &Render,
    node: sva_formula::NodeId,
    representation: Representation,
) -> Result<Answer, EngineError> {
    let closed = render.tys.ty(node).is_closed_form();
    let profile = render.config.profile.name;
    if representation == Representation::Flops {
        return Ok(Answer::whole(
            Output::Flops(Box::new(crate::flops::tree_at(render, node))),
            Source::Exact,
            profile,
            None,
        ));
    }
    if representation == Representation::Arguments {
        return Ok(Answer::whole(
            Output::Arguments(arguments_under(render, node)),
            Source::Exact,
            profile,
            None,
        ));
    }
    if representation == Representation::Bindings {
        return Ok(Answer::whole(
            Output::Bindings(render.bindings.get(&node).cloned().unwrap_or_default()),
            Source::Exact,
            profile,
            None,
        ));
    }
    match representation.consumes(closed) {
        Consumes::ClosedForm => {
            let found;
            let sum = match render.symbolic.get(&node) {
                Some(held) => held,
                None => match refs::spectral_sum_of(&render.tys, node, render.tys.var(node)) {
                    Ok(held) => {
                        found = held;
                        &found
                    }
                    Err(left) => return measured_instead(render, node, representation, left),
                },
            };
            let (value, listed, source) = match exact(render, node, representation, sum) {
                Err(left) => return measured_instead(render, node, representation, left),
                Ok(held) => held,
            };
            Ok(Answer {
                value,
                source,
                profile,
                rate: None,
                tail_db: listed.tail_db(),
                dropped: listed.dropped,
            })
        }
        _ => {
            let buffer = render
                .buffers
                .get(&node)
                .ok_or_else(|| unmaterialized(render, node, representation))?;
            Ok(Answer::whole(
                measured(render, node, representation, buffer)?,
                Source::Measured,
                profile,
                Some(render.config.rate),
            ))
        }
    }
}

/// A closed form no atom sum reaches answers no reading symbolically. An envelope is the one
/// FORMAT 9.3 already names off `sample(...)`, so it is measured here rather than refused.
fn measured_instead(
    render: &Render,
    node: sva_formula::NodeId,
    representation: Representation,
    left: EngineError,
) -> Result<Answer, EngineError> {
    match representation {
        Representation::Envelope { frame_secs } if left.code() == "cast.left_algebra" => {
            off_the_grid(render, node, frame_secs)
        }
        _ => Err(left),
    }
}

/// The samples this node reaches: the buffer the render holds, or the collapse it would have run,
/// down to FORMAT 9.1's row 4 over the written closed form.
fn on_the_grid(render: &Render, node: sva_formula::NodeId) -> Result<Buffer, EngineError> {
    if let Some(held) = render.buffers.get(&node) {
        return Ok(held.clone());
    }
    let (rate, horizon) = (render.config.rate, render.config.horizon);
    let profile = &render.config.profile;
    let written = refs::substituted_closed_form(&render.tys, node);
    let composed;
    let sum = match render.symbolic.get(&node) {
        Some(held) => Some(held),
        None => match refs::spectral_sum_of(&render.tys, node, render.tys.var(node)) {
            Ok(held) => {
                composed = held;
                Some(&composed)
            }
            Err(left) if written.is_none() => return Err(left),
            Err(_) => None,
        },
    };
    let taken = match sum {
        Some(sum) => sva_samples::of_spectral_sum_or_point(
            sum,
            written.as_ref(),
            rate,
            horizon,
            profile,
            AliasScore::NotAsked,
        ),
        None => sva_samples::render(
            written
                .as_ref()
                .expect("a node with no sum answers off the closed form written above"),
            rate,
            horizon,
            profile,
            AliasScore::NotAsked,
        ),
    };
    taken
        .map(|(buffer, _)| buffer)
        .map_err(|e| refused(render, node, e.code(), e.to_string()))
}

fn off_the_grid(
    render: &Render,
    node: sva_formula::NodeId,
    frame_secs: Option<f64>,
) -> Result<Answer, EngineError> {
    let rate = render.config.rate;
    let buffer = on_the_grid(render, node)?;
    Ok(Answer::whole(
        Output::Envelope(envelope::trace(
            buffer.plane(0),
            f64::from(rate),
            buffer.origin_secs,
            frame_secs.unwrap_or(DEFAULT_FRAME_SECS),
        )),
        Source::Measured,
        render.config.profile.name,
        Some(rate),
    ))
}

fn exact(
    render: &Render,
    node: sva_formula::NodeId,
    representation: Representation,
    sum: &SpectralSum,
) -> Result<(Output, Listed, Source), EngineError> {
    let source = Source::Exact;
    let enumerated = || {
        listed(
            render,
            node,
            sum,
            render.config.profile.floor(AUDIBLE_CEILING_HZ),
            render.config.profile.half_lsb(),
        )
    };
    let (value, listed) = match representation {
        Representation::Lines => {
            let listed = enumerated()?;
            let held = lines(render, node, sum.var, &listed.atoms)?;
            (Output::Lines(held), listed)
        }
        Representation::Spectrum {
            frame_secs: Some(_),
            ..
        } => {
            return Err(refused(
                render,
                node,
                "engine.observation_needs_samples",
                "a closed form has no frames to take a spectrum across".to_string(),
            ));
        }
        // FORMAT 14.1: a pair's spectrum is its whole line list; no estimate, no peaks.
        Representation::Spectrum {
            frame_secs: None, ..
        } => {
            let listed = enumerated()?;
            let mut held = lines(render, node, sum.var, &listed.atoms)?;
            held.sort_by(|a, b| a.hz.total_cmp(&b.hz));
            (Output::Lines(held), listed)
        }
        Representation::Atoms => {
            let listed = enumerated()?;
            let found = listed.atoms.iter().map(sketch_atom).collect();
            (Output::Atoms(found), listed)
        }
        // A pair states its partials; `max_notes` is the question, not a cut answer.
        Representation::Pitch { max_notes, .. } => {
            let listed = enumerated()?;
            let held = lines(render, node, sum.var, &listed.atoms)?;
            let mut found: Vec<Peak> = held
                .iter()
                .filter(|l| l.hz > 0.0)
                .map(|l| Peak {
                    hz: l.hz,
                    db: 20.0 * amplitude_at(&held, l.hz).log10(),
                })
                .collect();
            found.sort_by(|a, b| b.db.total_cmp(&a.db));
            let frame = PitchFrame {
                t_secs: render.config.horizon.start_secs,
                notes: pitch::name_peaks(&found, max_notes),
            };
            (Output::Pitch(vec![frame]), listed)
        }
        Representation::Derivative => (Output::Symbolic(Box::new(d_dt(sum))), Listed::NONE),
        Representation::Envelope { .. } => {
            let held = envelope(sum).map_err(|left| {
                EngineError::of_closed_form(
                    &left.refusal(),
                    render.tys.locate(left.origin),
                    "read the envelope off sample(...) for a measured one",
                )
            })?;
            (Output::Symbolic(Box::new(held.squared)), Listed::NONE)
        }
        other => return Err(not_a_closed_form(render, node, other)),
    };
    Ok((value, listed, source))
}

/// Every atom the spectral sum stands for, beside the terms its series truncated away or left
/// above the ceiling: `atoms` alone holds only the terms already written out.
struct Listed {
    atoms: Vec<SpectralAtom>,
    dropped: Vec<Line>,
}

impl Listed {
    /// A reading that enumerates no series leaves nothing out.
    const NONE: Listed = Listed {
        atoms: Vec::new(),
        dropped: Vec::new(),
    };

    /// FORMAT 9.3: the tail is the loudest line left out against the loudest one kept, so
    /// it is one ratio over the whole answer, never a per-series one carried upward.
    fn tail_db(&self) -> Option<f64> {
        let loudest = |set: &mut dyn Iterator<Item = f64>| set.fold(0.0f64, f64::max);
        let gone = loudest(&mut self.dropped.iter().map(|l| l.amp.abs()));
        let kept = loudest(&mut self.atoms.iter().map(|a| a.c.abs()));
        (gone > 0.0 && kept > 0.0).then(|| 20.0 * (gone / kept).log10())
    }
}

/// Each lane's series enumerated under the band a rate-free reading is taken in. A series
/// whose term no line closed form reads yields nothing, and nothing is not an exact empty answer.
fn listed(
    render: &Render,
    node: sva_formula::NodeId,
    sum: &SpectralSum,
    floor_db: f64,
    precision: f64,
) -> Result<Listed, EngineError> {
    let mut held = Listed::NONE;
    for lane in &sum.lanes {
        held.atoms.extend(lane.clone().expanded().atoms);
        for series in &lane.series {
            let Some(found) = line_atoms(series, AUDIBLE_CEILING_HZ, floor_db, precision) else {
                return Err(unenumerable(render, node));
            };
            held.atoms.extend(found.atoms);
            held.dropped.extend(found.dropped);
        }
    }
    Ok(held)
}

/// The exact line list of a pair: in `t` a bare turning exponential, in `f` the delta it
/// duals to. A list every other atom was dropped from answers a spectrum this node has not.
fn lines(
    render: &Render,
    node: sva_formula::NodeId,
    var: Var,
    atoms: &[SpectralAtom],
) -> Result<Vec<Line>, EngineError> {
    atoms
        .iter()
        .map(|a| line_of(var, a).ok_or_else(|| not_a_line(render, node, var, a)))
        .collect()
}

fn line_of(var: Var, a: &SpectralAtom) -> Option<Line> {
    match (var, a.sing) {
        (Var::F, Singular::Delta { at, order: 0 }) => Some(Line::bare(at, a.c)),
        (Var::T, Singular::Regular) => {
            if a.poly > 0 || a.gauss.is_some() || a.ind.is_some() || a.pole.is_some() {
                return None;
            }
            match a.exp {
                // A constant turns at no rate, which is the line at zero hertz.
                None => Some(Line::bare(0.0, a.c)),
                Some(e) if e.sigma == 0.0 => Some(Line::bare(e.omega / std::f64::consts::TAU, a.c)),
                Some(_) => None,
            }
        }
        _ => None,
    }
}

/// What gave this atom a width; a turning exponential is the line itself and never among it.
fn widening(a: &SpectralAtom) -> Vec<&'static str> {
    let mut held = Vec::new();
    if a.poly > 0 {
        held.push("a polynomial");
    }
    if a.exp.is_some_and(|e| e.sigma != 0.0) {
        held.push("a decaying exponential");
    }
    if a.gauss.is_some() {
        held.push("a Gaussian");
    }
    if a.pole.is_some() {
        held.push("a pole");
    }
    if held.is_empty() {
        held.push("a delta");
    }
    held
}

/// Convolved into a line, each shape beside the turning exponential answers a band.
fn not_a_line(
    render: &Render,
    node: sva_formula::NodeId,
    var: Var,
    atom: &SpectralAtom,
) -> EngineError {
    let mut at = render.tys.locate(atom.origin);
    if at.node.is_empty() {
        at = Located::at(render.tys.name(node), None);
    }
    let message = match atom.ind {
        _ if var == Var::F => "this term spreads over `f` rather than standing at one \
                               frequency, and a line in `f` is a delta"
            .to_string(),
        Some(window) => format!(
            "this term is windowed to [{}s, {}s), and a windowed line is that line convolved \
             with the window's transform, which has a width and is no line",
            window.l.value(),
            window.r.value()
        ),
        None => format!(
            "this term carries {}, and a line is a bare turning exponential: each of those \
             convolves the line with a shape of its own width",
            widening(atom).join(", ")
        ),
    };
    EngineError::refused(Diagnostic {
        code: "read.lines_need_unwindowed_lines".to_string(),
        message,
        location: at,
        help: "`--as atoms` states each term as it stands, and `--as lines` of the node under \
               the window or envelope lists the lines it multiplies"
            .to_string(),
    })
}

/// The amplitude a partial sounds at: a real wave carries it in a conjugate pair, half in
/// each, and a measured peak reads the pair's sum.
fn amplitude_at(held: &[Line], hz: f64) -> f64 {
    held.iter()
        .filter(|l| (l.hz - hz).abs() <= f64::EPSILON * hz.abs() || l.hz == -hz)
        .map(|l| l.amp.abs())
        .sum()
}

/// One atom as the six factors it is present in, which is what `atoms` answers with.
pub fn sketch_atom(a: &SpectralAtom) -> String {
    let factors: Vec<&'static str> = a.present().iter().map(|f| f.as_str()).collect();
    format!("{} x {}", a.c.abs(), factors.join(" times "))
}

fn measured(
    render: &Render,
    node: sva_formula::NodeId,
    representation: Representation,
    buffer: &Buffer,
) -> Result<Output, EngineError> {
    Ok(match representation {
        Representation::Alias { oversample } => {
            let reference = oversampled(render, behind(render, node)?, oversample)?;
            Output::Alias(Box::new(worst_alias(buffer, &reference, oversample)))
        }
        Representation::Ledger { depth } => {
            Output::Ledger(attributed(render, node, depth, 0..buffer.len())?)
        }
        other => {
            return off_buffer(buffer, other).map_err(|fault| match fault {
                NoReading::NeedsAGraph => not_a_reading(render, node, other),
                NoReading::TooNarrow { need, held } => too_narrow(render, node, need, held),
            });
        }
    })
}

/// Why a buffer answered nothing: the reading needs the tree the node was built from, or
/// the buffer is narrower than the reading needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoReading {
    NeedsAGraph,
    TooNarrow { need: usize, held: usize },
}

/// Every reading a buffer answers on its own, with no graph behind it.
pub fn off_buffer(buffer: &Buffer, representation: Representation) -> Result<Output, NoReading> {
    let sr = f64::from(buffer.rate);
    let start = buffer.origin_secs;
    let plane = buffer.plane(0);
    Ok(match representation {
        Representation::Samples => Output::Samples(Box::new(buffer.clone())),
        Representation::Spectrum {
            max_peaks,
            frame_secs,
        } => Output::Spectrum(Box::new(spectrum::analyze(
            plane, sr, max_peaks, frame_secs,
        ))),
        Representation::Envelope { frame_secs } => Output::Envelope(envelope::trace(
            plane,
            sr,
            start,
            frame_secs.unwrap_or(DEFAULT_FRAME_SECS),
        )),
        Representation::Derivative => Output::Samples(Box::new(difference(buffer))),
        Representation::Pitch {
            max_notes,
            frame_secs,
        } => Output::Pitch(pitch::track(plane, sr, start, frame_secs, max_notes)),
        Representation::Formants {
            max_formants,
            frame_secs,
        } => Output::Formants(formants::track(
            plane,
            sr,
            start,
            frame_secs,
            formants::default_order(sr),
            max_formants,
        )),
        Representation::Stereo { frame_secs } => {
            if buffer.width < 2 {
                return Err(NoReading::TooNarrow {
                    need: 2,
                    held: buffer.width,
                });
            }
            let planes: Vec<&[f64]> = (0..buffer.width).map(|c| buffer.plane(c)).collect();
            Output::Stereo(Box::new(stereo::analyze(
                &planes,
                buffer.width,
                sr,
                start,
                frame_secs,
            )))
        }
        Representation::Bands => Output::Bands(Box::new(bands::analyze(plane, sr, start))),
        Representation::Crest => Output::Crest(Box::new(crest::analyze(plane, sr))),
        Representation::Loudness => {
            let planes: Vec<&[f64]> = (0..buffer.width).map(|c| buffer.plane(c)).collect();
            Output::Loudness(Box::new(loudness::analyze(&planes, sr, start)))
        }
        _ => return Err(NoReading::NeedsAGraph),
    })
}

/// Every held buffer under the target, its own refs beside it, so the reading can share the
/// target's energy down the tree it was built from. A ref the closed form adds stands there as what
/// it contributed to the node reading it; one no addend isolates is left unattributed.
fn attributed(
    render: &Render,
    node: sva_formula::NodeId,
    depth: usize,
    range: std::ops::Range<usize>,
) -> Result<Vec<sva_samples::LedgerEntry>, EngineError> {
    let mut buffers = std::collections::BTreeMap::new();
    let mut deps = std::collections::BTreeMap::new();
    let mut kinds = std::collections::BTreeMap::new();
    for (id, buffer) in &render.buffers {
        let name = render.tys.name(*id).to_string();
        let read = refs_read(render, *id)?
            .into_iter()
            .map(|op| render.tys.name(op).to_string())
            .collect();
        deps.insert(name.clone(), read);
        kinds.insert(name.clone(), sva_samples::SignalKind::Audio);
        buffers.insert(name, buffer.clone());
    }
    let mut contributed_by = std::collections::BTreeMap::new();
    for (parent, child) in edges_under(render, node, depth)? {
        if let Some(held) = contributed(render, parent, child)? {
            contributed_by.insert(render.tys.name(child).to_string(), held);
        }
    }
    Ok(sva_samples::measure::ledger::attribute(
        &buffers,
        &contributed_by,
        &deps,
        &kinds,
        render.tys.name(node),
        range,
        depth,
    ))
}

/// The ledger over the part of a render `over` names: the same tree and the same shares,
/// each summed over those samples alone, as a render over that window alone would sum them.
pub fn ledger_over(
    render: &Render,
    node: sva_formula::NodeId,
    depth: usize,
    over: Horizon,
) -> Result<Answer, EngineError> {
    let representation = Representation::Ledger { depth };
    let buffer = render
        .buffers
        .get(&node)
        .ok_or_else(|| unmaterialized(render, node, representation))?;
    Ok(Answer::whole(
        Output::Ledger(attributed(render, node, depth, buffer.span_of(over))?),
        Source::Measured,
        render.config.profile.name,
        Some(render.config.rate),
    ))
}

/// What every instance under `node` was lowered with, `node` first, then breadth first. The
/// walk is over what each node was lowered to, so it needs no buffer.
fn arguments_under(render: &Render, node: sva_formula::NodeId) -> Vec<crate::Arguments> {
    let under = |id: sva_formula::NodeId| -> Vec<sva_formula::NodeId> {
        match render.tys.value(id) {
            Value::ClosedForm(form) => refs::nodes_in(&form.body),
            Value::Read { source, .. } | Value::Cast(_, source) => vec![*source],
            Value::Op { args, .. } => args.clone(),
            Value::Filter {
                x, cutoff, q, gain, ..
            } => vec![*x, *cutoff, *q, *gain],
            Value::SelfAt(_) | Value::Grid(_) | Value::Solver(_) => Vec::new(),
        }
    };
    let mut names: Vec<&str> = Vec::new();
    let mut seen = std::collections::BTreeSet::from([node]);
    let mut level = vec![node];
    while !level.is_empty() {
        let mut next = Vec::new();
        for at in level {
            let name = render.tys.name(at);
            if !names.contains(&name) {
                names.push(name);
            }
            next.extend(under(at).into_iter().filter(|c| seen.insert(*c)));
        }
        level = next;
    }
    names
        .into_iter()
        .filter_map(|name| render.tys.arguments(name).cloned())
        .collect()
}

/// The tree a ledger walks; a node several read is attributed to the first to reach it.
fn edges_under(
    render: &Render,
    node: sva_formula::NodeId,
    depth: usize,
) -> Result<Vec<(sva_formula::NodeId, sva_formula::NodeId)>, EngineError> {
    let mut seen = std::collections::BTreeSet::from([node]);
    let (mut level, mut out) = (vec![node], Vec::new());
    for _ in 0..depth {
        let mut next = Vec::new();
        for parent in level {
            for child in refs_read(render, parent)? {
                if seen.insert(child) {
                    out.push((parent, child));
                    next.push(child);
                }
            }
        }
        level = next;
    }
    Ok(out)
}

/// Every ref one node reads: a closed form's own, and a sampled node's buffer slots.
fn refs_read(
    render: &Render,
    node: sva_formula::NodeId,
) -> Result<Vec<sva_formula::NodeId>, EngineError> {
    match render.tys.ty(node).is_closed_form() {
        true => Ok(crate::schedule::read_operands(&render.tys, node)),
        false => crate::render::slots::refs_read(render, node),
    }
}

/// What one ref contributed to the node reading it, at that node's own offset and window:
/// its own addend, where the closed form adds its refs. Two under one product have no addend apiece
/// and no share either, so that edge is left unattributed.
fn contributed(
    render: &Render,
    parent: sva_formula::NodeId,
    child: sva_formula::NodeId,
) -> Result<Option<Buffer>, EngineError> {
    if !render.tys.ty(parent).is_closed_form() {
        return crate::render::slots::contributed(render, parent, child);
    }
    let Value::ClosedForm(form) = render.tys.value(parent) else {
        return Ok(None);
    };
    Ok(separable(&form.body, child)
        .then(|| collapsed(render, parent, &alone(&form.body, child), form.var))
        .flatten())
}

fn collapsed(
    render: &Render,
    parent: sva_formula::NodeId,
    body: &sva_formula::Body,
    var: Var,
) -> Option<Buffer> {
    let sum = refs::spectral_sum_of_body(&render.tys, parent, body, var).ok()?;
    sva_samples::collapse::of_spectral_sum(
        &sum,
        render.config.rate,
        render.config.horizon,
        &render.config.profile,
        AliasScore::NotAsked,
    )
    .ok()
    .map(|(buffer, _)| buffer)
}

fn alone(f: &sva_formula::Body, child: sva_formula::NodeId) -> sva_formula::Body {
    match f {
        sva_formula::Body::Node(id) if *id != child => {
            sva_formula::Body::Const(sva_formula::C64::ZERO)
        }
        other => sva_formula::closed_form::map_children(other, |p| {
            sva_formula::Part::new(p.origin, alone(&p.body, child))
        }),
    }
}

/// Whether silencing every other ref leaves this one's contribution standing.
fn separable(f: &sva_formula::Body, child: sva_formula::NodeId) -> bool {
    let parts = sva_formula::closed_form::children(f);
    match f {
        sva_formula::Body::Add(_) => parts.iter().all(|p| separable(&p.body, child)),
        _ => {
            let mut holding = parts.iter().filter(|p| !refs::nodes_in(&p.body).is_empty());
            match (holding.next(), holding.next()) {
                (None, _) => true,
                (Some(only), None) => separable(&only.body, child),
                _ => !refs::nodes_in(f).contains(&child),
            }
        }
    }
}

/// The closed form an alias score oversamples: the node itself where it is one, and the operand of
/// the `sample(...)` that collapsed it where it is not.
fn behind(render: &Render, node: sva_formula::NodeId) -> Result<sva_formula::NodeId, EngineError> {
    if render.tys.ty(node).is_closed_form() {
        return Ok(node);
    }
    match render.tys.value(node) {
        crate::typing::Value::Cast(crate::cast::Cast::Sample, source) => Ok(*source),
        _ => Err(refused(
            render,
            node,
            "engine.alias_needs_a_closed_form",
            "an alias score is a render against the same closed form oversampled, and this node \
             is samples with no closed form behind it"
                .to_string(),
        )),
    }
}

/// Each component scored against its own reference, and `worst` says which one answers.
fn worst_alias(buffer: &Buffer, reference: &Buffer, oversample: u32) -> sva_samples::Alias {
    debug_assert_eq!(
        buffer.width, reference.width,
        "the reference is the same form at another rate"
    );
    let sr = f64::from(buffer.rate);
    let width = buffer.width.min(reference.width);
    sva_samples::worst_alias((0..width).map(|c| {
        measure_alias(
            buffer.plane(c),
            reference.plane(c),
            oversample as usize,
            sr,
            buffer.origin_secs,
        )
    }))
    .expect("a buffer holds at least one component")
}

/// The same closed form read at a multiple of the rate, which is what an alias score is against.
fn oversampled(
    render: &Render,
    node: sva_formula::NodeId,
    oversample: u32,
) -> Result<Buffer, EngineError> {
    let rate = render.config.rate * oversample;
    let taken = match refs::spectral_sum_of(&render.tys, node, Var::T) {
        Ok(sum) => sva_samples::of_spectral_sum(
            &sum,
            rate,
            render.config.horizon,
            &render.config.profile,
            AliasScore::NotAsked,
        ),
        Err(e) => match refs::substituted_closed_form(&render.tys, node) {
            Some(form) => sva_samples::render(
                &form,
                rate,
                render.config.horizon,
                &render.config.profile,
                AliasScore::NotAsked,
            ),
            None => return Err(e),
        },
    };
    taken.map(|(buffer, _)| buffer).map_err(|e| {
        EngineError::refused(Diagnostic {
            code: e.code().to_string(),
            message: e.to_string(),
            location: Located::at(render.tys.name(node), None),
            help: "an alias score needs a closed form to oversample".to_string(),
        })
    })
}

/// A first difference on the grid, which is what a derivative is once the closed form is gone.
fn difference(buffer: &Buffer) -> Buffer {
    let step = f64::from(buffer.rate);
    let planes = (0..buffer.width)
        .map(|c| {
            let plane = buffer.plane(c);
            plane
                .iter()
                .enumerate()
                .map(|(i, x)| match i {
                    0 => 0.0,
                    _ => (x - plane[i - 1]) * step,
                })
                .collect()
        })
        .collect();
    let mut out = Buffer::of_planes(buffer.rate, planes);
    out.origin_secs = buffer.origin_secs;
    out
}

fn refused(render: &Render, node: sva_formula::NodeId, code: &str, message: String) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(render.tys.name(node), None),
        help: "ask for a reading this representation answers".to_string(),
    })
}

fn unenumerable(render: &Render, node: sva_formula::NodeId) -> EngineError {
    refused(
        render,
        node,
        "read.series_not_enumerable",
        "a series whose term is no line lists no lines, and an empty list would read as \
         a node with none"
            .to_string(),
    )
}

fn not_a_closed_form(render: &Render, node: sva_formula::NodeId, r: Representation) -> EngineError {
    refused(
        render,
        node,
        "engine.observation_needs_samples",
        format!(
            "`{}` reads samples, and this node is a closed form",
            r.name()
        ),
    )
}

fn not_a_reading(render: &Render, node: sva_formula::NodeId, r: Representation) -> EngineError {
    refused(
        render,
        node,
        "engine.observation_not_wired",
        format!("`{}` takes no reading off a buffer here", r.name()),
    )
}

fn too_narrow(render: &Render, node: sva_formula::NodeId, need: usize, held: usize) -> EngineError {
    refused(
        render,
        node,
        "type.width_mismatch",
        format!("this reading needs {need} components and the node holds {held}"),
    )
}

fn unmaterialized(render: &Render, node: sva_formula::NodeId, r: Representation) -> EngineError {
    refused(
        render,
        node,
        "engine.not_materialized",
        format!("`{}` reads samples this render never held", r.name()),
    )
}

/// One reading off a buffer nothing rendered — an external file, say — under the profile the
/// caller names. Every representation needing the graph behind it refuses here.
pub fn answer_buffer(
    name: &str,
    buffer: &Buffer,
    representation: Representation,
    profile: &'static str,
) -> Result<Answer, EngineError> {
    let refused = |code: &str, message: String| {
        EngineError::refused(Diagnostic {
            code: code.to_string(),
            message,
            location: Located::at(name, None),
            help: "ask for a reading a buffer answers on its own".to_string(),
        })
    };
    let value = off_buffer(buffer, representation).map_err(|fault| match fault {
        NoReading::NeedsAGraph => refused(
            "engine.observation_needs_a_graph",
            format!(
                "`{}` reads the tree a node was built from, and a file has none",
                representation.name()
            ),
        ),
        NoReading::TooNarrow { need, held } => refused(
            "type.width_mismatch",
            format!("this reading needs {need} components and the file holds {held}"),
        ),
    })?;
    Ok(Answer::whole(
        value,
        Source::Measured,
        profile,
        Some(buffer.rate),
    ))
}
