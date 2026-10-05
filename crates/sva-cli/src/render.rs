// Concern: runs one render or one file analysis and routes every reading it answered | Non-concern: argv (args/), the JSON shape (sva-core) | IO: (RenderArgs or AnalyzeArgs) -> JSON or CliError

use std::path::Path;

use sva_core::{
    Answer, Asked, CliError, Diagnostic, Job, Output, Printed, Report, SAMPLE_LIMIT, Session, Tier,
    cwd, execute_over, query_data,
};
use sva_engine::{Buffer, PSYCHOACOUSTIC_V1, answer_buffer, cache_log};

use crate::args::{AnalyzeArgs, RenderArgs};
use crate::destination::{Framing, refuse_inside, refuse_replacing, write};
use crate::directory::Directory;
use crate::store::wait;
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
    let mut warnings = Vec::new();
    let tier = crate::store::opened(&args.cache, &mut warnings)?;
    let answered = answered(args, &dir, &target, tier, &mut warnings);
    crate::store::persisted(tier, &mut warnings);
    answered.map(|data| success_envelope(&data, &warnings))
}

/// The render's `data`.
fn answered(
    args: &RenderArgs,
    dir: &Path,
    target: &str,
    tier: &Tier<Directory>,
    warnings: &mut Vec<Diagnostic>,
) -> Result<String, CliError> {
    let source = sva_ast::Dir::at(dir);
    let logs = std::env::var("SVA_LOG").is_ok_and(|level| level == "debug");
    let job = Job {
        until: args.until.as_deref(),
        rate: args.rate,
        bits: args.bits,
        asked: &args.asked,
        ..Job::over(&source, target)
    };
    let rendered = wait(execute_over(job, tier, &mut Session::default()))?;

    let rate = rendered.config.rate;
    let stats = rendered.render.cache_stats.as_ref();
    if let Some(why) = stats.and_then(|stats| stats.unstaged.as_deref()) {
        let message = format!("this render stopped writing what it computed: {why}");
        warnings.push(crate::store::warning("store.unstaged", message));
    }
    if let (true, Some(stats)) = (logs, stats) {
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

    Ok(query_data(&Report {
        target: &args.target,
        rate,
        bits: Some(bits),
        interval,
        profile: rendered.config.profile.name,
        label: rendered.label(),
        written: &written,
        answers: &answers,
        limit: Some(SAMPLE_LIMIT),
    }))
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
    for dest in args.asked.iter().filter_map(|a| a.dest.as_deref()) {
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
    let (answers, written) = routed(&args.asked, taken, &framing)?;
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
            limit: Some(SAMPLE_LIMIT),
        }),
        &[],
    ))
}

/// Whether the next parse would walk this directory, not whether it parses cleanly today.
fn is_composition(dir: &Path) -> bool {
    dir.join(sva_core::ROOT).is_file() || dir.join(sva_ast::VARIABLES).is_dir()
}
