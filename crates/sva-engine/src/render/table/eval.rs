// Concern: computes one value over the segments asked of it, from the values it reads | Non-concern: choosing the segments, keeping them | IO: (value, segments, read values) -> priced flops

use std::borrow::Cow;

use sva_samples::{Buffer, Extent, Label, Machine, Profile, Tape, Window, stft};

use super::demand::{Need, images};
use super::segments::Segments;
use super::value::{Held, Kind, Program, Value, finite};
use super::values::Values;
use crate::error::{Diagnostic, EngineError, Located};

/// `done` holds every value `value` reads; a run marks its state at each of `marks` it passes.
/// What it cost, and the waves its rows turned.
pub(crate) fn compute(
    value: &mut Value,
    need: &Need,
    (done, marks): (&Values, &Marks),
    profile: &Profile,
) -> Result<(u128, u128), EngineError> {
    let (mut priced, mut waves) = (0, 0);
    for segment in need.compute.iter() {
        if let Kind::Rows(rows) = &value.kind {
            waves += rows.work(segment.start, segment.end).1;
        }
        priced += match &value.kind {
            Kind::Rows(_) => rows(value, segment)?,
            Kind::Program(program) if program.stateful() => {
                stepped(value, segment, need.restart, (done, marks))?
            }
            Kind::Program(_) => program(value, segment, done)?,
            Kind::Frames { .. } => frames(value, segment, done)?,
            Kind::Istft => istft(value, segment, done, profile)?,
            Kind::Spectrum(_) => spectrum(value, segment, profile)?,
            Kind::Stored { .. } => {
                let live = *value
                    .reads
                    .first()
                    .expect("a stored value short of its readers");
                value.hold(samples_of(done, live, segment));
                0
            }
        };
        value.evaluated.push(segment);
    }
    Ok((priced, waves))
}

fn rows(value: &mut Value, segment: Extent) -> Result<u128, EngineError> {
    let Kind::Rows(rows) = &value.kind else {
        unreachable!("a value of rows");
    };
    let planes = rows
        .planes(segment.start, segment.end)
        .map_err(|e| collapse_refused(&value.name, &e))?;
    finite(&value.name, planes.iter().flatten())?;
    let priced = rows.work(segment.start, segment.end).0;
    let mut buffer = Buffer::of_planes(value.grid.rate, planes);
    buffer.start = segment.start;
    value.hold(buffer);
    Ok(priced)
}

/// Every value a slot reads, viewed over what `over` reads of it.
fn views<'a>(value: &Value, program: &Program, over: Extent, done: &'a Values) -> Vec<View<'a>> {
    images(program, over)
        .into_iter()
        .zip(&value.reads)
        .map(|(image, read)| View::of(done, *read, image.hull()))
        .collect()
}

/// A value's held samples, through every alias between, each moving it by its shift.
enum View<'a> {
    Run(&'a Tape, &'a Value, i64),
    Held(Cow<'a, Buffer>, &'a Value, i64),
}

impl<'a> View<'a> {
    fn of(done: &'a Values, mut at: usize, over: Extent) -> View<'a> {
        let mut by = 0;
        while let Some((read, shift)) = done[at].alias() {
            (at, by) = (read, by + shift);
        }
        let value = &done[at];
        match &value.held {
            Held::Run(tape) => View::Run(tape, value, by),
            _ => View::Held(value.window(over.shifted(by)), value, by),
        }
    }

    fn window(&self) -> Window<'_> {
        match self {
            View::Run(tape, value, by) => tape.within(value.support()).shifted(*by),
            View::Held(buffer, value, by) => Window::of(buffer, value.support())
                .folded(value.period)
                .shifted(*by),
        }
    }
}

fn program(value: &mut Value, segment: Extent, done: &Values) -> Result<u128, EngineError> {
    let Kind::Program(program) = &value.kind else {
        unreachable!("a program");
    };
    let held = views(value, program, segment, done);
    let windows: Vec<Window> = held.iter().map(View::window).collect();
    let mut machine = Machine::over(&program.spanned, segment.start)
        .map_err(|e| sample_refused(&value.name, &e))?;
    let mut tape = Tape::new(value.width, segment.len(), segment.start);
    machine
        .run_to(segment.end, &windows, &mut tape)
        .map_err(|e| sample_refused(&value.name, &e))?;
    finite(&value.name, tape.planes().iter().flatten())?;
    let priced = program.spanned.ops(segment.start, segment.end);
    value.hold(tape.into_buffer(value.grid.rate));
    Ok(priced)
}

/// Where a run keeps its state: each switch, and a ladder `every` samples from its start.
#[derive(Clone, Debug, Default)]
pub(crate) struct Marks {
    pub(crate) at: Vec<i64>,
    pub(crate) every: Option<usize>,
}

impl Marks {
    fn within(&self, base: i64, from: i64, to: i64) -> Vec<i64> {
        let mut out: Vec<i64> = self
            .at
            .iter()
            .copied()
            .filter(|m| from < *m && *m < to)
            .collect();
        if let Some(every) = self.every {
            let step = every as i64;
            let mut at = base + ((from - base).div_euclid(step) + 1) * step;
            while at < to {
                out.push(at);
                at += step;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// On from where its machine stands, or from its start again, marking its state on the way.
fn stepped(
    value: &mut Value,
    segment: Extent,
    restart: bool,
    (done, marks): (&Values, &Marks),
) -> Result<u128, EngineError> {
    let (name, width) = (value.name.clone(), value.width);
    let Kind::Program(program) = &mut value.kind else {
        unreachable!("a program");
    };
    let Held::Run(tape) = &mut value.held else {
        unreachable!("a stateful value holds a run");
    };
    if restart || program.machine.is_none() {
        *tape = Tape::new(width, 0, segment.start);
        program.marks.clear();
        program.machine = Some(
            Machine::over(&program.spanned, segment.start)
                .map_err(|e| sample_refused(&name, &e))?,
        );
    }
    let from = tape.end();
    let held: Vec<View> = images(program, Extent::new(from, segment.end))
        .into_iter()
        .zip(&value.reads)
        .map(|(image, read)| View::of(done, *read, image.hull()))
        .collect();
    let windows: Vec<Window> = held.iter().map(View::window).collect();
    let machine = program.machine.as_mut().expect("a machine");
    let base = program.start.expect("a stateful program").min(tape.base());
    for at in marks.within(base, from, segment.end) {
        machine
            .run_to(at, &windows, tape)
            .map_err(|e| sample_refused(&name, &e))?;
        program.marks.insert(at, machine.state());
    }
    machine
        .run_to(segment.end, &windows, tape)
        .map_err(|e| sample_refused(&name, &e))?;
    let grown = (tape.end() - from) as usize;
    finite(
        &name,
        tape.planes().iter().flat_map(|p| &p[p.len() - grown..]),
    )?;
    Ok(program.spanned.ops(from, segment.end))
}

fn frames(value: &mut Value, segment: Extent, done: &Values) -> Result<u128, EngineError> {
    let Kind::Frames { window, hop } = value.kind else {
        unreachable!("frames");
    };
    let source = samples_of(done, value.reads[0], segment);
    let frames =
        stft::forward(&source, window, hop).map_err(|e| sample_refused(&value.name, &e))?;
    let count = segment.len().div_ceil(hop.max(1)) as u128;
    value.held = Held::Frames(Some(Box::new(frames)));
    Ok(count * sva_samples::collapse::transform_flops(window.max(1)))
}

fn istft(
    value: &mut Value,
    segment: Extent,
    done: &Values,
    profile: &Profile,
) -> Result<u128, EngineError> {
    let Held::Frames(Some(frames)) = &done[value.reads[0]].held else {
        return Err(refused(
            &value.name,
            "cast.istft_needs_frames",
            "the frames this reads were never built",
        ));
    };
    let (inverse, label) = stft::inverse(frames, profile);
    let count = frames.frames as u128;
    let priced = count * sva_samples::collapse::transform_flops(frames.window.max(1));
    value.hold(inverse.over(segment, inverse.extent()));
    value.label = Some(label);
    Ok(priced)
}

fn spectrum(value: &mut Value, segment: Extent, profile: &Profile) -> Result<u128, EngineError> {
    let Kind::Spectrum(sum) = &value.kind else {
        unreachable!("a spectrum");
    };
    let rate = value.grid.rate;
    let plan = sva_samples::collapse::plan::of(sum, rate, segment, profile, segment.len())
        .map_err(|e| collapse_refused(&value.name, &e))?;
    let priced = plan.flops(rate, segment);
    let (buffer, label): (Buffer, Label) = sva_samples::of_spectral_sum(
        sum,
        rate,
        segment,
        profile,
        sva_samples::AliasScore::NotAsked,
    )
    .map_err(|e| collapse_refused(&value.name, &e))?;
    finite(&value.name, buffer.planes.iter().flatten())?;
    value.hold(buffer);
    value.label = Some(label);
    Ok(priced)
}

/// `at`'s program with another renderer, over `over` from where its state starts: what a
/// ledger reads of one slot, the others silenced.
pub(crate) fn rerun(
    values: &Values,
    at: usize,
    renderer: &sva_samples::NodeRenderer,
    over: Extent,
) -> Result<Buffer, EngineError> {
    let value = &values[at];
    let Kind::Program(program) = &value.kind else {
        unreachable!("a program");
    };
    let from = program
        .start
        .map_or(over.start, |start| start.min(over.start));
    let live: Vec<Extent> = value.reads.iter().map(|r| values[*r].support()).collect();
    let spanned = sva_samples::Spanned::new(renderer, &program.layout, (from, over.end), &live)
        .map_err(|e| sample_refused(&value.name, &e))?;
    let rerun = Program {
        renderer: std::sync::Arc::new(renderer.clone()),
        spanned: std::sync::Arc::new(spanned),
        layout: std::sync::Arc::clone(&program.layout),
        start: program.start,
        own: program.own,
        alias: None,
        machine: None,
        marks: Default::default(),
    };
    let span = Extent::new(from, over.end);
    let held = views(value, &rerun, span, values);
    let windows: Vec<Window> = held.iter().map(View::window).collect();
    let mut machine =
        Machine::over(&rerun.spanned, from).map_err(|e| sample_refused(&value.name, &e))?;
    let mut tape = Tape::new(value.width, span.len(), from);
    machine
        .run_to(over.end, &windows, &mut tape)
        .map_err(|e| sample_refused(&value.name, &e))?;
    let buffer = tape.into_buffer(value.grid.rate);
    Ok(buffer.over(over, buffer.extent()))
}

/// A value's samples over `over`, through every alias between.
pub(crate) fn samples_of(values: &Values, at: usize, over: Extent) -> Buffer {
    match values[at].alias() {
        Some((read, by)) => {
            let mut held = samples_of(values, read, over.shifted(by));
            held.start = over.start;
            held
        }
        None => values[at].samples(over),
    }
}

/// What `need` would cost, priced as `compute` pays it.
pub(crate) fn price(value: &Value, need: &Segments) -> u128 {
    need.iter()
        .map(|segment| match &value.kind {
            Kind::Rows(rows) => rows.work(segment.start, segment.end).0,
            Kind::Program(program) if program.alias.is_some() => 0,
            Kind::Program(program) => program.spanned.ops(segment.start, segment.end),
            Kind::Frames { window, hop } => {
                segment.len().div_ceil((*hop).max(1)) as u128
                    * sva_samples::collapse::transform_flops((*window).max(1))
            }
            Kind::Istft | Kind::Spectrum(_) => segment.len() as u128,
            Kind::Stored { .. } => 0,
        })
        .sum()
}

pub(crate) fn sample_refused(name: &str, e: &sva_samples::SampleError) -> EngineError {
    let help = match e {
        sva_samples::SampleError::ArgumentOutOfRange { .. } => {
            "keep the parameter in its range at every sample; `sva-cli builtins` names each \
             argument's range"
        }
        _ => "write the node so the grid can hold it",
    };
    EngineError::refused(Diagnostic {
        code: e.code().to_string(),
        message: e.to_string(),
        location: Located::at(name, None),
        help: help.to_string(),
    })
}

pub(crate) fn collapse_refused(name: &str, e: &sva_samples::CollapseError) -> EngineError {
    EngineError::refused(Diagnostic {
        code: e.code().to_string(),
        message: e.to_string(),
        location: Located::at(name, None),
        help: e.help().to_string(),
    })
}

fn refused(name: &str, code: &str, message: &str) -> EngineError {
    EngineError::refused(Diagnostic {
        code: code.to_string(),
        message: message.to_string(),
        location: Located::at(name, None),
        help: "write the node so the grid can hold it".to_string(),
    })
}
