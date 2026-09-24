// Concern: runs one render or one file analysis and routes every reading it answered | Non-concern: argv (args/), the JSON shape (sva-core) | IO: (RenderArgs or AnalyzeArgs) -> JSON or CliError

use std::path::Path;

use sva_core::{
    Answer, Asked, CacheReport, CliError, Horizon, Job, Output, Report, SAMPLE_LIMIT, cwd, execute,
    query_data, window_for,
};
use sva_engine::{
    Buffer, Cache, DEFAULT_FRAME_SECS, DiskCache, PSYCHOACOUSTIC_V1, Representation, answer_buffer,
};

use crate::args::{AnalyzeArgs, RenderArgs};
use crate::destination::{Framing, refuse_inside, refuse_replacing, write, write_analysis};
use crate::wav::{SampleEncoding, read_channels};
use sva_core::success_envelope;

/// A target renders FROM itself, so only its dependency subtree is evaluated and its own
/// extent decides the length.
pub fn render(args: &RenderArgs) -> Result<String, CliError> {
    let dir = crate::composition::composition(args.dir.as_deref())?;
    // Judged before the first is opened: a refusal on the last leaves none of them on disk.
    for asked in &args.asked {
        if let Some(dest) = asked.dest.as_deref() {
            refuse_inside(&dir, dest)?;
            refuse_replacing(dest, args.confirm)?;
        }
    }
    let store = args.cache.then(DiskCache::discover).flatten();
    let cache = store.as_ref().map(|c| c as &dyn Cache);
    let source = sva_ast::Dir::at(&dir);
    let rendered = execute(Job {
        target: args.target.as_deref(),
        from: args.from,
        until: args.to,
        sample_rate: args.sample_rate,
        cache,
        reading: args.node.as_deref(),
        representations: args.asked.iter().map(|a| a.representation).collect(),
        // A named target pulls its own closure; nothing it never reads is parsed at all.
        reaching: args.target.is_some(),
        flop_budget: args.flop_budget,
        ..Job::over(&source)
    })?;

    let node = args.node.clone().unwrap_or_else(|| rendered.target.clone());
    let framing = Framing {
        target: rendered.target.clone(),
        rate: rendered.config.rate,
        horizon: rendered.config.horizon,
        profile: rendered.config.profile.name,
        encoding: match args.pcm16 {
            true => SampleEncoding::Pcm16,
            false => SampleEncoding::Float,
        },
        skim: args.skim,
        replace: args.confirm,
    };

    let mut taken = Vec::with_capacity(args.asked.len());
    for asked in &args.asked {
        let answer = rendered.answer(&node, asked.representation)?;
        taken.push(match args.brief {
            true => briefed(answer),
            false => answer,
        });
    }
    let (answers, written) = routed(&args.asked, taken, &framing)?;

    let report = cache.map(|c| CacheReport {
        dir: c
            .dir()
            .map_or_else(|| "memory".to_string(), |d| d.display().to_string()),
        stats: rendered
            .render
            .cache_stats
            .clone()
            .expect("a render handed a store records every lookup it made"),
        held_bytes: c.held_bytes(),
        max_bytes: c.max_bytes(),
        evicted_bytes: c.evicted_bytes(),
        faults: c.faults(),
    });
    Ok(success_envelope(
        &query_data(&Report {
            target: &rendered.target,
            rate: rendered.config.rate,
            horizon: rendered.config.horizon,
            profile: rendered.config.profile.name,
            label: rendered.label(),
            written: &written,
            cache: report.as_ref(),
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
            skim: args.skim,
        }),
        &[],
    ))
}

/// What stays in the envelope, and what left it for a file.
type Routed<'a> = (Vec<(String, Answer)>, Vec<(String, &'a Path)>);

/// Every reading is taken before any file is opened, so a later refusal writes nothing.
fn routed<'a>(
    asked: &'a [Asked],
    taken: Vec<Answer>,
    framing: &Framing,
) -> Result<Routed<'a>, CliError> {
    let mut answers = Vec::new();
    let mut written = Vec::new();
    for (one, answer) in asked.iter().zip(taken) {
        match one.dest.as_deref() {
            None => answers.push((one.name.clone(), answer)),
            Some(dest) => {
                write(&one.name, &answer, dest, framing)?;
                written.push((one.name.clone(), dest));
            }
        }
    }
    Ok((answers, written))
}

/// `--brief`: a node that did not clip is not news, so a ledger keeps only the ones that did.
fn briefed(answer: Answer) -> Answer {
    let Output::Ledger(entries) = answer.value else {
        return answer;
    };
    Answer {
        value: Output::Ledger(
            entries
                .into_iter()
                .filter(|e| e.clipped == Some(true))
                .collect(),
        ),
        ..answer
    }
}

/// The same routing over a decoded external WAV instead of a rendered graph: the file's own
/// rate stands, and its own length is the horizon.
pub fn analyze(args: &AnalyzeArgs) -> Result<String, CliError> {
    let dir = cwd()?;
    let composition = is_composition(&dir);
    let destinations = args
        .asked
        .iter()
        .map(|a| a.dest.as_deref())
        .chain(args.analyses.iter().map(|(_, dest)| dest.as_deref()));
    for dest in destinations {
        let Some(dest) = dest else {
            continue;
        };
        if composition {
            refuse_inside(&dir, dest)?;
        }
        refuse_replacing(dest, args.confirm)?;
    }
    let (planes, rate) = read_channels(&args.path)?;
    let target = args.path.display().to_string();
    let held = Buffer::of_planes(
        rate,
        planes
            .iter()
            .map(|p| p.iter().map(|v| f64::from(*v)).collect())
            .collect(),
    );
    let asked_window = window_for(args.from, args.to, None)?;
    let duration = held.len() as f64 / f64::from(rate);
    let horizon = Horizon::secs(
        asked_window.start_secs,
        match asked_window.end_secs.is_finite() {
            true => asked_window.end_secs,
            false => duration,
        },
    );
    if horizon.span() <= 0.0 {
        return Err(CliError::Usage(format!(
            "a reading runs from {}s to {}s, which is no window; give --to past --from",
            horizon.start_secs, horizon.end_secs
        )));
    }
    // A window past the end asks for samples that do not exist.
    if horizon.start_secs >= duration || horizon.end_secs > duration {
        return Err(CliError::Usage(format!(
            "{} is {duration}s long, and the window asked for runs to {}s",
            args.path.display(),
            horizon.end_secs
        )));
    }
    crate::args::check_frame(&args.asked, rate)?;
    let buffer = sliced(&held, horizon);
    let framing = Framing {
        target: target.clone(),
        rate,
        horizon,
        profile: PSYCHOACOUSTIC_V1.name,
        encoding: SampleEncoding::Float,
        skim: false,
        replace: args.confirm,
    };

    let mut taken = Vec::with_capacity(args.asked.len());
    for asked in &args.asked {
        taken.push(
            answer_buffer(
                &target,
                &buffer,
                asked.representation,
                PSYCHOACOUSTIC_V1.name,
            )
            .map_err(CliError::Engine)?,
        );
    }
    let heard = analysed(args, &buffer, horizon)?;
    let (answers, mut written) = routed(&args.asked, taken, &framing)?;
    let mut analyses = Vec::new();
    for ((name, dest), value) in args.analyses.iter().zip(heard) {
        match dest.as_deref() {
            None => analyses.push((name.clone(), value)),
            Some(dest) => {
                write_analysis(name, &value, dest, &framing)?;
                written.push((name.clone(), dest));
            }
        }
    }
    Ok(success_envelope(
        &query_data(&Report {
            target: &target,
            rate,
            horizon,
            profile: PSYCHOACOUSTIC_V1.name,
            label: None,
            written: &written,
            cache: None,
            answers: &answers,
            analyses: &analyses,
            limit: Some(SAMPLE_LIMIT),
            skim: false,
        }),
        &[],
    ))
}

/// The readings `sva-analysis` answers off a buffer alone. Every field its `Request` names is
/// taken from this one file: a `.wav` carries no tempo and no node behind it.
fn analysed(
    args: &AnalyzeArgs,
    buffer: &Buffer,
    horizon: Horizon,
) -> Result<Vec<String>, CliError> {
    if args.analyses.is_empty() {
        return Ok(Vec::new());
    }
    let rate = f64::from(buffer.rate);
    let mono: Vec<f32> = buffer.plane(0).iter().map(|s| *s as f32).collect();
    let against = match &args.against {
        Some(path) => {
            let (planes, _) = read_channels(path)?;
            Some(planes.first().cloned().unwrap_or_default())
        }
        None => None,
    };
    let name = args.path.display().to_string();
    let read = |representation| {
        answer_buffer(&name, buffer, representation, PSYCHOACOUSTIC_V1.name).map(|a| a.value)
    };
    let envelope = match read(Representation::Envelope { frame_secs: None }) {
        Ok(Output::Envelope(frames)) => frames,
        _ => Vec::new(),
    };
    let image = match read(Representation::Stereo {
        frame_secs: DEFAULT_FRAME_SECS,
    }) {
        Ok(Output::Stereo(found)) => Some(*found),
        _ => None,
    };
    let request = sva_analysis::Request {
        samples: &mono,
        sample_rate: rate,
        start_secs: horizon.start_secs,
        frame_secs: DEFAULT_FRAME_SECS,
        tempo: None,
        envelope: &envelope,
        stereo: image.as_ref(),
        against: against.as_deref(),
        gated: false,
        input_envelope: None,
    };
    let mut out = Vec::with_capacity(args.analyses.len());
    for (name, _) in &args.analyses {
        out.push(
            sva_analysis::run(name, &request)
                .map_err(|sva_analysis::AnalysisError(why)| CliError::Usage(why))?,
        );
    }
    Ok(out)
}

/// Whether the next parse would walk this directory, not whether it parses cleanly today.
fn is_composition(dir: &Path) -> bool {
    dir.join(sva_core::ROOT).is_file() || dir.join(sva_ast::VARIABLES).is_dir()
}

/// A file has no horizon of its own, so the window is taken off the samples it holds.
fn sliced(buffer: &Buffer, horizon: Horizon) -> Buffer {
    let rate = f64::from(buffer.rate);
    let start = ((horizon.start_secs * rate).round().max(0.0) as usize).min(buffer.len());
    let end = ((horizon.end_secs * rate).round().max(0.0) as usize).clamp(start, buffer.len());
    let mut out = Buffer::of_planes(
        buffer.rate,
        (0..buffer.width)
            .map(|c| buffer.plane(c)[start..end].to_vec())
            .collect(),
    );
    out.origin_secs = horizon.start_secs;
    out
}
