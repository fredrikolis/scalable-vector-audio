// Concern: runs one render or one file analysis and routes every reading it answered | Non-concern: argv (args/), the JSON shape (sva-core) | IO: (RenderArgs or AnalyzeArgs) -> JSON or CliError

use std::path::Path;

use sva_core::{
    Answer, Asked, CliError, Job, Output, Printed, Report, SAMPLE_LIMIT, cwd, execute, query_data,
};
use sva_engine::{
    Buffer, Cache, DEFAULT_CACHE_BYTES, DEFAULT_FRAME_SECS, PSYCHOACOUSTIC_V1, Representation,
    answer_buffer, cache_log,
};

use crate::args::{AnalyzeArgs, RenderArgs};
use crate::destination::{Framing, refuse_inside, refuse_replacing, write, write_analysis};
use crate::wav::{SampleEncoding, read_channels};
use sva_core::success_envelope;

/// Only the nodes the target reaches are read; its interval and `--until` decide its range.
pub fn render(args: &RenderArgs) -> Result<String, CliError> {
    let here = crate::composition::composition()?;
    let (dir, target) = crate::composition::located(&here, &args.target)?;
    // Judged before the first is opened: a refusal on the last leaves none of them on disk.
    for asked in &args.asked {
        if let Some(dest) = asked.dest.as_deref() {
            refuse_inside(&dir, dest)?;
            refuse_replacing(dest, args.confirm)?;
        }
    }
    let source = sva_ast::Dir::at(&dir);
    // The log reports a store's lookups, so a logged render is given one; its samples are the same bits.
    let logs = std::env::var("SVA_LOG").is_ok_and(|level| level == "debug");
    let store = logs.then(|| Cache::holding(DEFAULT_CACHE_BYTES));
    let rendered = execute(Job {
        cache: store.as_ref(),
        until: args.until.as_deref(),
        rate: args.rate,
        bits: args.bits,
        asked: &args.asked,
        flop_budget: args.flop_budget,
        ..Job::over(&source, &target)
    })?;

    let rate = rendered.config.rate;
    if let Some(stats) = &rendered.render.cache_stats {
        eprint!("{}", cache_log(stats, rate));
    }
    let bits = rendered.config.profile.precision_bits;
    let interval = rendered
        .render
        .range
        .map(|r| (r.start_secs(rate), r.end as f64 / f64::from(rate)));
    let framing = Framing {
        target: args.target.clone(),
        rate,
        bits: Some(bits),
        interval,
        profile: rendered.config.profile.name,
        encoding: SampleEncoding::of(bits),
        replace: args.confirm,
    };

    let mut taken = Vec::with_capacity(args.asked.len());
    for asked in &args.asked {
        let node = asked.node.as_deref().unwrap_or(&rendered.target);
        let answer = rendered.answer(node, asked.representation)?;
        taken.push(match asked.brief {
            true => briefed(answer),
            false => answer,
        });
    }
    let (answers, written) = routed(&args.asked, taken, &framing)?;

    Ok(success_envelope(
        &query_data(&Report {
            target: &args.target,
            rate,
            bits: Some(bits),
            interval,
            profile: rendered.config.profile.name,
            label: rendered.label(),
            written: &written,
            answers: &answers,
            analyses: &[],
            limit: Some(SAMPLE_LIMIT),
        }),
        &[],
    ))
}

/// What stays in the envelope, and what left it for a file.
type Routed<'a> = (Vec<Printed>, Vec<(String, &'a Path)>);

/// Every reading is taken before any file is opened, so a later refusal writes nothing.
fn routed<'a>(
    asked: &'a [Asked],
    taken: Vec<Answer>,
    framing: &Framing,
) -> Result<Routed<'a>, CliError> {
    let mut answers = Vec::new();
    let mut written = Vec::new();
    for (one, answer) in asked.iter().zip(taken) {
        let printed = Printed {
            name: one.name.clone(),
            answer,
            skim: one.skim,
        };
        match one.dest.as_deref() {
            None => answers.push(printed),
            Some(dest) => {
                write(&printed, dest, framing)?;
                written.push((one.name.clone(), dest));
            }
        }
    }
    Ok((answers, written))
}

/// `ledger(brief=1)`: a node that did not clip is not news, so a ledger keeps only the ones
/// that did.
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
/// rate stands, and it is read whole.
pub fn analyze(args: &AnalyzeArgs) -> Result<String, CliError> {
    let dir = cwd()?;
    let composition = is_composition(&dir);
    let destinations = args
        .asked
        .iter()
        .map(|a| a.dest.as_deref())
        .chain(args.analyses.iter().map(|a| a.dest.as_deref()));
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
    crate::args::check_frame(&args.asked, rate)?;
    let buffer = held;
    let interval = Some((0.0, buffer.len() as f64 / f64::from(rate)));
    let framing = Framing {
        target: target.clone(),
        rate,
        bits: None,
        interval,
        profile: PSYCHOACOUSTIC_V1.name,
        encoding: SampleEncoding::Float,
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
    let heard = analysed(args, &buffer)?;
    let (answers, mut written) = routed(&args.asked, taken, &framing)?;
    let mut analyses = Vec::new();
    for (analysis, value) in args.analyses.iter().zip(heard) {
        match analysis.dest.as_deref() {
            None => analyses.push((analysis.name.clone(), value)),
            Some(dest) => {
                write_analysis(&analysis.name, &value, dest, &framing)?;
                written.push((analysis.name.clone(), dest));
            }
        }
    }
    Ok(success_envelope(
        &query_data(&Report {
            target: &target,
            rate,
            bits: None,
            interval,
            profile: PSYCHOACOUSTIC_V1.name,
            label: None,
            written: &written,
            answers: &answers,
            analyses: &analyses,
            limit: Some(SAMPLE_LIMIT),
        }),
        &[],
    ))
}

/// The readings `sva-analysis` answers off a buffer alone. Every field its `Request` names is
/// taken from this one file: a `.wav` carries no tempo and no node behind it.
fn analysed(args: &AnalyzeArgs, buffer: &Buffer) -> Result<Vec<String>, CliError> {
    if args.analyses.is_empty() {
        return Ok(Vec::new());
    }
    let rate = f64::from(buffer.rate);
    let mono: Vec<f32> = buffer.plane(0).iter().map(|s| *s as f32).collect();
    let against = match args.analyses.iter().find_map(|a| a.against.as_deref()) {
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
        start_secs: 0.0,
        frame_secs: DEFAULT_FRAME_SECS,
        tempo: None,
        envelope: &envelope,
        stereo: image.as_ref(),
        against: against.as_deref(),
        gated: false,
        input_envelope: None,
    };
    let mut out = Vec::with_capacity(args.analyses.len());
    for analysis in &args.analyses {
        out.push(
            sva_analysis::run(&analysis.name, &request)
                .map_err(|sva_analysis::AnalysisError(why)| CliError::Usage(why))?,
        );
    }
    Ok(out)
}

/// Whether the next parse would walk this directory, not whether it parses cleanly today.
fn is_composition(dir: &Path) -> bool {
    dir.join(sva_core::ROOT).is_file() || dir.join(sva_ast::VARIABLES).is_dir()
}
