// Concern: computes one value over the segments asked of it, from the values it reads | Non-concern: choosing the segments, keeping them | IO: (value, segments, read values) -> priced flops

use std::borrow::Cow;

use sva_samples::{Buffer, Extent, Machine, Profile, SampleView, stft};

use super::demand::{Need, images};
use super::segments::Segments;
use super::value::{Holding, Kind, MachineRun, Value, finite};
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
        priced += cost(value, segment, done);
        match &value.kind {
            Kind::Rows(_) => rows(value, segment)?,
            Kind::MachineRun(machine_run) if machine_run.stateful() => {
                stepped(value, segment, need.restart, (done, marks))?
            }
            Kind::MachineRun(_) => run_machine(value, segment, done)?,
            Kind::Frames { .. } => frames(value, segment, done)?,
            Kind::Istft => istft(value, segment, done, profile)?,
            Kind::Resident { .. } => {
                let live = *value
                    .reads
                    .first()
                    .expect("a stored value short of its readers");
                value.hold(samples_of(done, live, segment));
            }
        }
        evaluated(&mut value.evaluated, segment);
    }
    Ok((priced, waves))
}

/// What computing `value` over `segment` costs: the one price both the budget and the count
/// read.
fn cost(value: &Value, segment: Extent, values: &Values) -> u128 {
    match &value.kind {
        Kind::Rows(rows) => rows.work(segment.start, segment.end).0,
        Kind::MachineRun(machine_run) if machine_run.alias.is_some() => 0,
        Kind::MachineRun(machine_run) => machine_run.spanned.ops(segment.start, segment.end),
        Kind::Frames { window, hop } => stft::flops(segment.len(), *window, *hop),
        Kind::Istft => match values[value.reads[0]].kind {
            Kind::Frames { window, hop } => stft::flops(segment.len(), window, hop),
            _ => 0,
        },
        Kind::Resident { .. } => 0,
    }
}

/// A segment continuing the last one computed extends it, so a value pulled block by block
/// keeps one segment per unbroken run, however long it plays.
fn evaluated(held: &mut Vec<Extent>, segment: Extent) {
    match held.last_mut() {
        Some(last) if last.end == segment.start => last.end = segment.end,
        _ => held.push(segment),
    }
}

fn rows(value: &mut Value, segment: Extent) -> Result<(), EngineError> {
    let Kind::Rows(rows) = &value.kind else {
        unreachable!("a value of rows");
    };
    let planes = rows
        .planes(segment.start, segment.end)
        .map_err(|e| collapse_refused(&value.name, &e))?;
    finite(&value.name, planes.iter().flatten())?;
    let mut buffer = Buffer::of_planes(value.grid.rate, planes);
    buffer.start = segment.start;
    value.hold(buffer);
    Ok(())
}

/// Every value a slot reads, viewed over what `over` reads of it.
fn views<'a>(
    value: &Value,
    machine_run: &MachineRun,
    over: Extent,
    done: &'a Values,
) -> Vec<View<'a>> {
    images(machine_run, over)
        .into_iter()
        .zip(&value.reads)
        .map(|(image, read)| View::of(done, *read, image.hull()))
        .collect()
}

/// A value's held samples, through every alias between, each moving it by its shift.
struct View<'a>(Cow<'a, Buffer>, &'a Value, i64);

impl<'a> View<'a> {
    fn of(done: &'a Values, mut at: usize, over: Extent) -> View<'a> {
        let mut by = 0;
        while let Some((read, shift)) = done[at].alias() {
            (at, by) = (read, by + shift);
        }
        let value = &done[at];
        View(value.samples_over(over.shifted(by)), value, by)
    }

    fn sample_view(&self) -> SampleView<'_> {
        let View(buffer, value, by) = self;
        buffer.within(value.support()).shifted(*by)
    }
}

fn run_machine(value: &mut Value, segment: Extent, done: &Values) -> Result<(), EngineError> {
    let Kind::MachineRun(machine_run) = &value.kind else {
        unreachable!("a machine run");
    };
    let held = views(value, machine_run, segment, done);
    let views: Vec<SampleView> = held.iter().map(View::sample_view).collect();
    let mut machine = Machine::over(&machine_run.spanned, segment.start)
        .map_err(|e| sample_refused(&value.name, &e))?;
    let mut own = Buffer::empty(value.grid.rate, value.width, segment.len(), segment.start);
    machine
        .run_to(segment.end, &views, (&mut own, segment.start))
        .map_err(|e| sample_refused(&value.name, &e))?;
    finite(&value.name, own.planes.iter().flatten())?;
    value.hold(own);
    Ok(())
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
) -> Result<(), EngineError> {
    let (name, width) = (value.name.clone(), value.width);
    let Kind::MachineRun(machine_run) = &mut value.kind else {
        unreachable!("a machine run");
    };
    let rate = value.grid.rate;
    let Holding::Run { samples, origin } = &mut value.holding else {
        unreachable!("a stateful value holds a run");
    };
    if restart || machine_run.machine.is_none() {
        (*samples, *origin) = (Buffer::empty(rate, width, 0, segment.start), segment.start);
        machine_run.marks.clear();
        machine_run.machine = Some(
            Machine::over(&machine_run.spanned, segment.start)
                .map_err(|e| sample_refused(&name, &e))?,
        );
    }
    let from = samples.end();
    debug_assert_eq!(
        from, segment.start,
        "demand asks a stateful value on from where its run ends, so `cost` prices what runs"
    );
    let held: Vec<View> = images(machine_run, Extent::new(from, segment.end))
        .into_iter()
        .zip(&value.reads)
        .map(|(image, read)| View::of(done, *read, image.hull()))
        .collect();
    let views: Vec<SampleView> = held.iter().map(View::sample_view).collect();
    let machine = machine_run.machine.as_mut().expect("a machine");
    let base = machine_run
        .start
        .expect("a stateful machine run")
        .min(samples.start);
    for at in marks.within(base, from, segment.end) {
        machine
            .run_to(at, &views, (samples, *origin))
            .map_err(|e| sample_refused(&name, &e))?;
        machine_run.marks.insert(at, machine.state());
    }
    machine
        .run_to(segment.end, &views, (samples, *origin))
        .map_err(|e| sample_refused(&name, &e))?;
    let grown = (samples.end() - from) as usize;
    finite(
        &name,
        samples.planes.iter().flat_map(|p| &p[p.len() - grown..]),
    )
}

fn frames(value: &mut Value, segment: Extent, done: &Values) -> Result<(), EngineError> {
    let Kind::Frames { window, hop } = value.kind else {
        unreachable!("frames");
    };
    let source = samples_of(done, value.reads[0], segment);
    let frames =
        stft::forward(&source, window, hop).map_err(|e| sample_refused(&value.name, &e))?;
    value.holding = Holding::Frames(Some(std::sync::Arc::new(frames)));
    Ok(())
}

fn istft(
    value: &mut Value,
    segment: Extent,
    done: &Values,
    profile: &Profile,
) -> Result<(), EngineError> {
    let Holding::Frames(Some(frames)) = &done[value.reads[0]].holding else {
        return Err(refused(
            &value.name,
            "cast.istft_needs_frames",
            "the frames this reads were never built",
        ));
    };
    let (inverse, label) = stft::inverse(frames, profile);
    value.hold(inverse.over(segment, inverse.extent()));
    value.label = Some(label);
    Ok(())
}

/// `at`'s machine run with another renderer, over `over` from where its state starts: what a
/// ledger reads of one slot, the others silenced.
pub(crate) fn rerun(
    values: &Values,
    at: usize,
    renderer: &sva_samples::NodeRenderer,
    over: Extent,
) -> Result<Buffer, EngineError> {
    let value = &values[at];
    let Kind::MachineRun(machine_run) = &value.kind else {
        unreachable!("a machine run");
    };
    let from = machine_run
        .start
        .map_or(over.start, |start| start.min(over.start));
    let live: Vec<Extent> = value.reads.iter().map(|r| values[*r].support()).collect();
    let spanned = sva_samples::Spanned::new(renderer, &machine_run.layout, (from, over.end), &live)
        .map_err(|e| sample_refused(&value.name, &e))?;
    let rerun = MachineRun {
        renderer: std::sync::Arc::new(renderer.clone()),
        spanned: std::sync::Arc::new(spanned),
        layout: std::sync::Arc::clone(&machine_run.layout),
        start: machine_run.start,
        own: machine_run.own,
        alias: None,
        sources: std::sync::Arc::clone(&machine_run.sources),
        machine: None,
        marks: Default::default(),
    };
    let span = Extent::new(from, over.end);
    let held = views(value, &rerun, span, values);
    let views: Vec<SampleView> = held.iter().map(View::sample_view).collect();
    let mut machine =
        Machine::over(&rerun.spanned, from).map_err(|e| sample_refused(&value.name, &e))?;
    let mut own = Buffer::empty(value.grid.rate, value.width, span.len(), from);
    machine
        .run_to(over.end, &views, (&mut own, from))
        .map_err(|e| sample_refused(&value.name, &e))?;
    Ok(own.over(over, own.extent()))
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
pub(crate) fn price(value: &Value, need: &Segments, values: &Values) -> u128 {
    need.iter()
        .map(|segment| cost(value, segment, values))
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
