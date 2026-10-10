// Concern: takes one reading off the representation a node declares, a ledger edge by edge | Non-concern: naming the observations (query.rs), the arithmetic of one | IO: (&Render, node) -> Answer

use sva_formula::closed_form::Edge;
use sva_formula::spectral_sum::atom::{Singular, SpectralAtom};
use sva_formula::{Line, SpectralSum, Var, d_dt, envelope_read, line_atoms_read};
use sva_samples::measure::harmony::dissonance::Crowded;
use sva_samples::measure::harmony::{
    self, Framing, Moment, chords, dissonance, inharmonicity, measured as framed, tonnetz,
    voice_leading,
};
use sva_samples::measure::{modulation, periodicity, self_similarity, sliding};
use sva_samples::{
    Buffer, Consumes, Extent, Peak, PitchFrame, Profile, Source, measure::bands, measure::crest,
    measure::envelope, measure::formants, measure::loudness, measure::onsets, measure::pitch,
    measure::spectrum, measure::stereo, measure_alias,
};

use crate::error::{Diagnostic, EngineError, Located};
use crate::query::{Answer, Atom, DEFAULT_FRAME_SECS, Harmonic, Output, Representation};
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
                    Err(left) => return Err(unsummed(representation, left)),
                },
            };
            let (value, listed, source) = exact(render, node, representation, sum)
                .map_err(|left| not_exact(representation, left))?;
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
            if render.buffer(node).is_none() {
                return Err(unmaterialized(render, node, representation));
            }
            let buffer = render.output(node)?;
            Ok(Answer::whole(
                measured(render, node, representation, &buffer)?,
                Source::Measured,
                profile,
                Some(render.config.rate),
            ))
        }
    }
}

fn exact(
    render: &Render,
    node: sva_formula::NodeId,
    representation: Representation,
    sum: &SpectralSum,
) -> Result<(Output, Listed, Source), EngineError> {
    let source = Source::Exact;
    let enumerated = || listed(render, node, sum, &render.config.profile);
    let (value, listed) = match representation {
        Representation::Lines => {
            let listed = enumerated()?;
            let held = lines(render, node, sum.var, &listed.atoms)?;
            (Output::Lines(held), listed)
        }
        Representation::Spectrum {
            frame_secs: Some(_),
            ..
        } => return Err(unframed(render, node, "spectrum is its lines", "the lines")),
        Representation::Envelope {
            frame_secs: Some(_),
        } => {
            return Err(unframed(
                render,
                node,
                "envelope is its exact squared magnitude",
                "the exact envelope",
            ));
        }
        // A pair's spectrum is its whole line list; no estimate, no peaks.
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
            let found = listed.atoms.iter().map(|a| atom_of(sum.var, a)).collect();
            (Output::Atoms(found), listed)
        }
        // A pair states its partials; `max_notes` is the question, not a cut answer.
        Representation::Pitch { max_notes, .. } => {
            let listed = enumerated()?;
            let held = lines(render, node, sum.var, &listed.atoms)?;
            let mut found: Vec<Peak> = harmony::partials(held.iter().map(|l| (l.hz, l.amp)))
                .into_iter()
                .map(|p| Peak {
                    hz: p.hz,
                    db: 20.0 * p.amplitude.log10(),
                })
                .collect();
            found.sort_by(|a, b| b.db.total_cmp(&a.db));
            let start = render.read_from.ok_or_else(|| {
                refused(
                    render,
                    node,
                    "engine.not_materialized",
                    "an exact `pitch` frame starts at the sample the render reads its root \
                     from, and this render, not asked for `pitch`, decided none"
                        .to_string(),
                )
            })?;
            let frame = PitchFrame {
                t_secs: start as f64 / f64::from(render.config.rate),
                notes: pitch::name_peaks(&found, max_notes),
            };
            (Output::Pitch(vec![frame]), listed)
        }
        Representation::Derivative => {
            let slope = d_dt(sum).map_err(|left| {
                EngineError::of_closed_form(
                    &left.refusal(),
                    render.tys.locate(left.origin),
                    "read the derivative of a lower order or degree",
                )
            })?;
            (Output::Symbolic(Box::new(slope)), Listed::NONE)
        }
        Representation::Envelope { frame_secs: None } => {
            let held =
                refs::read_through(&render.tys, |t| envelope_read(sum, t)).map_err(|left| {
                    EngineError::of_closed_form(
                        &left.refusal(),
                        render.tys.locate(left.origin),
                        "read the envelope off sample(...) for a measured one",
                    )
                })?;
            (Output::Symbolic(Box::new(held.squared)), Listed::NONE)
        }
        Representation::Harmony {
            reading,
            frame_secs,
            floor_db,
        } => {
            let (moments, framing, listed) = exact_moments(render, node, sum, frame_secs)?;
            let value = harmonic(reading, &moments, framing, floor_db)
                .map_err(|held| crowded(render.tys.name(node), representation, held))?;
            (value, listed)
        }
        other => return Err(not_a_closed_form(render, node, other)),
    };
    Ok((value, listed, source))
}

/// Each instant on the frame grid over the range, its first component's partials exactly:
/// every atom a partial at its carrier, `omega/2pi`, its complex value there merged with the
/// others at that carrier, so a decay, polynomial, Gaussian or window weighs it as it stands.
fn exact_moments(
    render: &Render,
    node: sva_formula::NodeId,
    sum: &SpectralSum,
    frame_secs: f64,
) -> Result<(Vec<Moment>, Framing, Listed), EngineError> {
    let range = match (render.range, &render.unranged) {
        (Some(range), _) => range,
        (None, Some(refused)) => return Err(refused.clone()),
        (None, None) => unreachable!("a harmonic reading decides its range before it answers"),
    };
    let rate = f64::from(render.config.rate);
    let hop = ((frame_secs * rate).round() as usize).max(1);
    let first = SpectralSum::of(sum.var, sum.lanes.iter().take(1).cloned().collect());
    let listed = listed(render, node, &first, &render.config.profile)?;
    let instants: Vec<f64> = (range.start..range.end)
        .step_by(hop)
        .map(|n| n as f64 / rate)
        .collect();
    let moments = match sum.var {
        Var::F => {
            let held = lines(render, node, Var::F, &listed.atoms)?;
            let partials = harmony::partials(held.iter().map(|l| (l.hz, l.amp)));
            instants
                .iter()
                .map(|t| Moment {
                    t_secs: *t,
                    partials: partials.clone(),
                })
                .collect()
        }
        Var::T => instants
            .iter()
            .map(|t| {
                let carried = listed.atoms.iter().map(|a| {
                    let value = (!a.is_delta()).then(|| a.smooth_at(*t)).flatten();
                    let hz = a.exp.map_or(0.0, |e| e.omega / std::f64::consts::TAU);
                    value
                        .map(|v| (hz, v))
                        .ok_or_else(|| no_partial(render, node, a, *t))
                });
                Ok(Moment {
                    t_secs: *t,
                    partials: harmony::partials(carried.collect::<Result<Vec<_>, _>>()?),
                })
            })
            .collect::<Result<_, EngineError>>()?,
    };
    let framing = Framing {
        hop_secs: hop as f64 / rate,
        window: None,
    };
    Ok((moments, framing, listed))
}

/// One harmonic reading off a curve of partials, whichever source took them.
fn harmonic(
    reading: Harmonic,
    moments: &[Moment],
    framing: Framing,
    floor_db: f64,
) -> Result<Output, Crowded> {
    let chords = || chords::segment(moments, framing.hop_secs, floor_db);
    Ok(match reading {
        Harmonic::Dissonance { max_pairs } => Output::Dissonance(Box::new(dissonance::measure(
            moments, framing, floor_db, max_pairs,
        )?)),
        Harmonic::VoiceLeading { norm } => Output::VoiceLeading(Box::new(voice_leading::analyze(
            chords(),
            framing,
            floor_db,
            norm,
        ))),
        Harmonic::TonalDistance => {
            Output::TonalDistance(Box::new(tonnetz::analyze(chords(), framing, floor_db)))
        }
        Harmonic::Inharmonicity => {
            Output::Inharmonicity(Box::new(inharmonicity::measure(moments, framing, floor_db)))
        }
    })
}

/// An impulse stands at no frequency, and a pole has no value at its own instant.
fn no_partial(
    render: &Render,
    node: sva_formula::NodeId,
    atom: &SpectralAtom,
    t: f64,
) -> EngineError {
    let mut at = render.tys.locate(atom.origin);
    if at.node.is_empty() {
        at = Located::at(render.tys.name(node), None);
    }
    let message = match atom.is_delta() {
        true => "this term is an impulse in `t`, which stands at no one frequency, so it is \
                 no partial"
            .to_string(),
        false => {
            format!("this term has a pole at {t}s, an instant of the curve, where it has no value")
        }
    };
    EngineError::refused(Diagnostic {
        code: "read.no_partial_at_instant".to_string(),
        message,
        location: at,
        help: "a harmonic reading of `sample(...)` of it, as `sample(@x([0, 1s]))`, is \
               measured off frames of its samples instead"
            .to_string(),
    })
}

fn crowded(name: &str, representation: Representation, held: Crowded) -> EngineError {
    let reading = representation.name();
    EngineError::refused(Diagnostic {
        code: "read.too_many_partial_pairs".to_string(),
        message: format!(
            "at {}s, {} partials may stand over the floor, {} pairs, past the {} asked for",
            held.t_secs, held.partials, held.pairs, held.cap
        ),
        location: Located::at(name, None),
        help: format!(
            "ask for more pairs, as `{reading}(pairs={})`, or raise the floor so fewer partials \
             pair, as `{reading}(floor=-30db)`",
            held.pairs
        ),
    })
}

/// Each names the window that would read, and `longer` how to hand it a target that long.
fn unread(
    name: &str,
    representation: Representation,
    why: sliding::Unread,
    longer: impl Fn(f64) -> String,
) -> EngineError {
    let reading = representation.name();
    let (code, message, help) = match why {
        sliding::Unread::WindowUnderTwoPeriods {
            window_secs,
            least_secs,
        } => (
            "read.window_under_two_periods",
            format!(
                "a {window_secs}s window holds under two of the longest period `{reading}` \
                 reads, {}s",
                least_secs / 2.0
            ),
            format!(
                "ask a window of {least_secs}s or longer, as `{reading}(window={least_secs}s)`"
            ),
        ),
        sliding::Unread::TargetShorterThanWindow {
            held_secs,
            window_secs,
            least_secs,
        } => (
            "read.target_under_window",
            format!("the target holds {held_secs}s, under the {window_secs}s window"),
            format!(
                "{}, or ask a window as short as {least_secs}s, as \
                 `{reading}(window={least_secs}s)`",
                longer(window_secs)
            ),
        ),
    };
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message,
        location: Located::at(name, None),
        help,
    })
}

/// Names the cell that fits and, through `shorter`, how to hand it a target that short.
fn too_many_cells(
    name: &str,
    held: self_similarity::TooManyCells,
    shorter: impl Fn(f64) -> String,
) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "read.too_many_cells".to_string(),
        message: format!(
            "the target's {}s at cell={}s is {} cells a side, past the {} a matrix holds",
            held.held_secs, held.cell_secs, held.cells, held.cap
        ),
        location: Located::at(name, None),
        help: format!(
            "ask a coarser cell, as `self_similarity(cell={}s)`, or {}",
            held.least_cell_secs,
            shorter(held.most_secs)
        ),
    })
}

/// A closed form's envelope is exact or refused, never measured unasked: a refusal of its
/// sum says so, and names `sample(...)` as the way to ask for the measured one.
fn not_exact(representation: Representation, left: EngineError) -> EngineError {
    match (representation, left) {
        (Representation::Envelope { frame_secs: None }, EngineError::Refused(cause)) => {
            EngineError::refused(Diagnostic {
                message: format!("this closed form has no exact envelope: {}", cause.message),
                help: "a measured envelope is asked for explicitly: read `envelope` of \
                       `sample(...)` of it, as `sample(@x([0, 1s]))`"
                    .to_string(),
                ..*cause
            })
        }
        (_, left) => left,
    }
}

/// A closed form with no atom sum has no exact partials; the measured ones are asked for
/// explicitly, never taken in their place.
fn unsummed(representation: Representation, left: EngineError) -> EngineError {
    match (representation, left) {
        (Representation::Harmony { .. }, EngineError::Refused(cause)) => {
            EngineError::refused(Diagnostic {
                message: format!(
                    "`{}` of a closed form reads its exact partials, and it has none: {}",
                    representation.name(),
                    cause.message
                ),
                help: format!(
                    "a measured `{}` is asked for explicitly: read it of `sample(...)` of it, as \
                     `sample(@x([0, 1s]))`",
                    representation.name()
                ),
                ..*cause
            })
        }
        (_, left) => not_exact(representation, left),
    }
}

/// A closed form holds no frames, so a written `frame` is refused rather than ignored.
fn unframed(render: &Render, node: sva_formula::NodeId, is: &str, bare: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: "engine.observation_needs_samples".to_string(),
        message: format!("a closed form's {is}, and it has no frames to take it across"),
        location: Located::at(render.tys.name(node), None),
        help: format!("drop `frame` for {bare}, or read `sample(...)` of it to measure frames"),
    })
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

    /// The tail is the loudest line left out against the loudest one kept, so
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
    profile: &Profile,
) -> Result<Listed, EngineError> {
    let ceiling = profile.ceiling_hz;
    let band = (ceiling, profile.floor(ceiling), profile.half_lsb());
    let mut held = Listed::NONE;
    for lane in &sum.lanes {
        held.atoms.extend(lane.clone().expanded().atoms);
        for series in &lane.series {
            let found = refs::read_through(&render.tys, |t| line_atoms_read(series, band, t));
            let Some(found) = found else {
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
            if !a.poly.is_one() || a.gauss.is_some() || a.ind.is_some() || a.pole.is_some() {
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
    if !a.poly.is_one() {
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
        help: "`atoms` states each term as it stands, `lines` of the node under the window \
               or envelope lists the lines it multiplies, and `pitch` or `spectrum` of \
               `sample(...)` of it measures it frame by frame"
            .to_string(),
    })
}

/// One atom as the six factors it is present in, which is what `atoms` answers with.
pub fn sketch_atom(a: &SpectralAtom) -> String {
    let factors: Vec<&'static str> = a.present().iter().map(|f| f.as_str()).collect();
    format!("{} x {}", a.c.abs(), factors.join(" times "))
}

/// The atom's own fields, read off as it holds them; a constant turns at zero and decays at
/// none. Adding zero writes a negative zero as the zero it equals.
fn atom_of(var: Var, a: &SpectralAtom) -> Atom {
    let (sigma, omega, mu) = a.exp.map_or((0.0, 0.0, 0.0), |e| (e.sigma, e.omega, e.mu));
    let bounded = |edge: Edge| Some(edge.value() + 0.0).filter(|x| x.is_finite());
    Atom {
        var,
        sketch: sketch_atom(a),
        amp: a.c,
        turns: omega / std::f64::consts::TAU + 0.0,
        decay: 0.0 - sigma,
        decay_from: mu + 0.0,
        degree: a.poly.degree,
        polynomial_at: a.poly.at + 0.0,
        start: a.ind.and_then(|i| bounded(i.l)),
        end: a.ind.and_then(|i| bounded(i.r)),
    }
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
        Representation::Ledger { depth } => Output::Ledger(attributed(render, node, depth)?),
        other => {
            return off_buffer(buffer, other).map_err(|fault| match fault {
                NoReading::NeedsAGraph => not_a_reading(render, node, other),
                NoReading::TooNarrow { need, held } => too_narrow(render, node, need, held),
                NoReading::Crowded(held) => crowded(render.tys.name(node), other, held),
                NoReading::Unread(why) => unread(render.tys.name(node), other, why, |secs| {
                    format!(
                        "render an interval of at least {secs}s, as `@x([0, {secs}s])` writes one"
                    )
                }),
                NoReading::TooManyCells(held) => {
                    too_many_cells(render.tys.name(node), held, |secs| {
                        format!(
                            "render an interval of at most {secs}s, as `@x([0, {secs}s])` \
                             writes one"
                        )
                    })
                }
            });
        }
    })
}

/// Why a buffer answered nothing: the reading needs the tree the node was built from, or
/// the buffer is narrower than the reading needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoReading {
    NeedsAGraph,
    TooNarrow { need: usize, held: usize },
    Crowded(Crowded),
    Unread(sliding::Unread),
    TooManyCells(self_similarity::TooManyCells),
}

/// Every reading a buffer answers on its own, with no graph behind it.
pub fn off_buffer(buffer: &Buffer, representation: Representation) -> Result<Output, NoReading> {
    let sr = f64::from(buffer.rate);
    let start = buffer.origin_secs();
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
            &buffer.slices(),
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
            if buffer.width() < 2 {
                return Err(NoReading::TooNarrow {
                    need: 2,
                    held: buffer.width(),
                });
            }
            let planes: Vec<&[f64]> = (0..buffer.width()).map(|c| buffer.plane(c)).collect();
            Output::Stereo(Box::new(stereo::analyze(
                &planes,
                buffer.width(),
                sr,
                start,
                frame_secs,
            )))
        }
        Representation::Bands => Output::Bands(Box::new(bands::analyze(plane, sr, start))),
        Representation::Crest => Output::Crest(Box::new(crest::analyze(plane, sr))),
        Representation::Onsets => Output::Onsets(Box::new(onsets::detect(plane, sr, start))),
        Representation::Loudness => {
            let planes: Vec<&[f64]> = (0..buffer.width()).map(|c| buffer.plane(c)).collect();
            Output::Loudness(Box::new(loudness::analyze(&planes, sr, start)))
        }
        Representation::Harmony {
            reading,
            frame_secs,
            floor_db,
        } => {
            let (moments, framing) = framed::moments(plane, sr, start, frame_secs);
            harmonic(reading, &moments, framing, floor_db).map_err(NoReading::Crowded)?
        }
        Representation::Periodicity {
            frame_secs,
            window_secs,
            curve,
        } => Output::Periodicity(Box::new(
            periodicity::measure(plane, sr, start, frame_secs, window_secs, curve)
                .map_err(NoReading::Unread)?,
        )),
        Representation::Modulation {
            frame_secs,
            window_secs,
            bpm,
        } => Output::Modulation(Box::new(
            modulation::measure(plane, sr, start, frame_secs, window_secs, bpm)
                .map_err(NoReading::Unread)?,
        )),
        Representation::SelfSimilarity {
            frame_secs,
            cell_secs,
            smooth_secs,
            window_secs,
            floor_db,
            listed,
            curve,
        } => {
            let asked = self_similarity::Asked {
                hop_secs: frame_secs,
                cell_secs,
                smooth_secs,
                window_secs,
                floor_db,
                listed,
                curve,
            };
            Output::SelfSimilarity(Box::new(
                self_similarity::measure(plane, sr, start, asked)
                    .map_err(NoReading::TooManyCells)?,
            ))
        }
        _ => return Err(NoReading::NeedsAGraph),
    })
}

/// Every held buffer under the target over the root's range, its own refs beside it, so the
/// reading can share the target's energy down the tree it was built from. A ref the closed form
/// adds stands there as what it contributed to the node reading it; one no addend isolates is
/// left unattributed.
fn attributed(
    render: &Render,
    node: sva_formula::NodeId,
    depth: usize,
) -> Result<Vec<sva_samples::LedgerEntry>, EngineError> {
    let holds = |id: sva_formula::NodeId| render.buffers.contains_key(&id);
    let edges = edges_under(render, node, depth, &holds)?;
    let under: std::collections::BTreeSet<sva_formula::NodeId> = std::iter::once(node)
        .chain(edges.iter().map(|(_, child)| *child))
        .collect();
    let mut deps = std::collections::BTreeMap::new();
    let mut kinds = std::collections::BTreeMap::new();
    for id in render.buffers.keys().filter(|id| under.contains(id)) {
        let name = render.tys.name(*id).to_string();
        let read = refs_read(render, *id, &holds)?
            .into_iter()
            .map(|op| render.tys.name(op).to_string())
            .collect();
        deps.insert(name.clone(), read);
        kinds.insert(name, sva_samples::SignalKind::Audio);
    }
    let mut contributed_by = std::collections::BTreeMap::new();
    for (parent, child) in &edges {
        if !crate::render::slots::reads_held(render, *parent)? {
            return Err(unmaterialized(
                render,
                node,
                Representation::Ledger { depth },
            ));
        }
        if let Some(held) = contributed(render, *parent, *child)? {
            contributed_by.insert(render.tys.name(*child).to_string(), held);
        }
    }
    // A row's own samples are read where it reads rows itself or no addend isolates it.
    let readers: std::collections::BTreeSet<_> = edges.iter().map(|(parent, _)| *parent).collect();
    let mut buffers = std::collections::BTreeMap::new();
    for id in render.buffers.keys().filter(|id| under.contains(id)) {
        let name = render.tys.name(*id);
        if *id == node || readers.contains(id) || !contributed_by.contains_key(name) {
            buffers.insert(name.to_string(), own_samples(render, *id)?);
        }
    }
    let len = render.range.map_or(0, |range| range.len());
    Ok(sva_samples::measure::ledger::attribute(
        &buffers,
        &contributed_by,
        &deps,
        &kinds,
        render.tys.name(node),
        0..len,
        depth,
    ))
}

/// A row's own samples over the range, held or pulled.
fn own_samples(render: &Render, id: sva_formula::NodeId) -> Result<Buffer, EngineError> {
    let range = render.range.expect("a ledger reads a decided range");
    match render.buffer(id) {
        Some(_) => render.output(id),
        None => super::sampled(render, id, range),
    }
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
            Value::Solver { varying, .. } => varying.iter().map(|(_, a)| *a).collect(),
            Value::SelfAt { .. } | Value::Noise(_) => Vec::new(),
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
    holds: &dyn Fn(sva_formula::NodeId) -> bool,
) -> Result<Vec<(sva_formula::NodeId, sva_formula::NodeId)>, EngineError> {
    let mut seen = std::collections::BTreeSet::from([node]);
    let (mut level, mut out) = (vec![node], Vec::new());
    for _ in 0..depth {
        let mut next = Vec::new();
        for parent in level {
            for child in refs_read(render, parent, holds)? {
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

/// Every ref one node's renderer reads, one row per name.
fn refs_read(
    render: &Render,
    node: sva_formula::NodeId,
    holds: &dyn Fn(sva_formula::NodeId) -> bool,
) -> Result<Vec<sva_formula::NodeId>, EngineError> {
    crate::render::slots::refs_read(render, node, holds)
}

/// What one ref contributed to the node reading it, at that node's own offset and window: its
/// renderer with every other slot silenced. Two under one product have no addend apiece and no
/// share either, so that edge is left unattributed.
fn contributed(
    render: &Render,
    parent: sva_formula::NodeId,
    child: sva_formula::NodeId,
) -> Result<Option<Buffer>, EngineError> {
    let range = render.range.expect("a ledger reads a decided range");
    Ok(crate::render::slots::contributed(render, parent, child)?
        .map(|held| held.over(range, held.extent())))
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

/// The score a point sampling's label carries where a reading asks for one.
pub(super) fn alias_db(
    render: &Render,
    node: sva_formula::NodeId,
    buffer: &Buffer,
) -> Result<f64, EngineError> {
    let oversample = sva_samples::ALIAS_OVERSAMPLE as u32;
    let reference = oversampled(render, behind(render, node)?, oversample)?;
    Ok(worst_alias(buffer, &reference, oversample).asr_db)
}

/// Each component scored against its own reference; the worst answers.
fn worst_alias(buffer: &Buffer, reference: &Buffer, oversample: u32) -> sva_samples::Alias {
    debug_assert_eq!(
        buffer.width(),
        reference.width(),
        "the reference is the same form at another rate"
    );
    let sr = f64::from(buffer.rate);
    let width = buffer.width().min(reference.width());
    sva_samples::worst_alias((0..width).map(|c| {
        measure_alias(
            buffer.plane(c),
            reference.plane(c),
            oversample as usize,
            sr,
            buffer.origin_secs(),
        )
    }))
    .expect("a buffer holds at least one component")
}

/// The node read at a multiple of the rate by its own route: what an alias score is against.
fn oversampled(
    render: &Render,
    node: sva_formula::NodeId,
    oversample: u32,
) -> Result<Buffer, EngineError> {
    let range = render.range.expect("an alias score reads a decided range");
    let k = i64::from(oversample);
    let extent = Extent::new(range.start * k, range.end * k);
    super::finer(render, node, oversample, extent)
}

/// A first difference on the grid, which is what a derivative is once the closed form is gone.
fn difference(buffer: &Buffer) -> Buffer {
    let step = f64::from(buffer.rate);
    let planes = (0..buffer.width())
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
    out.start = buffer.start;
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

pub(super) fn unheld(render: &Render, node: sva_formula::NodeId) -> EngineError {
    refused(
        render,
        node,
        "engine.not_materialized",
        "this render held no samples of it".to_string(),
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
        NoReading::Crowded(held) => crowded(name, representation, held),
        NoReading::Unread(why) => unread(name, representation, why, |secs| {
            format!("analyze a file holding at least {secs}s")
        }),
        NoReading::TooManyCells(held) => too_many_cells(name, held, |secs| {
            format!("analyze a file holding at most {secs}s")
        }),
    })?;
    Ok(Answer::whole(
        value,
        Source::Measured,
        profile,
        Some(buffer.rate),
    ))
}
