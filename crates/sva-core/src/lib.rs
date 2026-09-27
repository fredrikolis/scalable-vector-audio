// Concern: the parse->tempo->render pipeline shared by both front ends | Non-concern: argv (sva-cli), JS bindings (sva-wasm) | IO: (a Source, a target) -> Rendered, a Stream or CliError

mod answer;
mod builtins;
mod cli_error;
pub mod json;
mod lint_code;
mod outline;
mod output;
mod query;
mod target;
mod tempo;
mod until;

pub use answer::{
    Printed, Report, SAMPLE_LIMIT, answer_json, label_json, query_data, stats_json, value_json,
    work_json,
};
pub use builtins::{Builtins, Callable, Crossing, builtins, builtins_data};
pub use cli_error::{CliError, LintViolation, lint_diagnostic};
pub use lint_code::LintCode;
pub use outline::outline_data;
pub use output::{Diagnostic, Severity, diagnostics_json, error_envelope, success_envelope};
pub use query::{
    Asked, Call, DEFAULT_LEDGER_DEPTH, DEFAULT_MAX_PEAKS, DEFAULT_OVERSAMPLE, REPRESENTATIONS,
    RETIRED, asked, call, calls, is_wav, retired, seconds, wav_path,
};
pub use target::{Edge, Target, target};
pub use tempo::{Tempo, refuse_unresolved_bars, resolved as tempo};
pub use until::until;

use std::path::Path;

use sva_ast::{Dir, Graph, Refusal, Source};
use sva_engine::{
    Ask, DEFAULT_SAMPLE_RATE, EngineError, Range, RenderConfig, StreamConfig, render,
};

pub use sva_engine::{Checkpoint, DEFAULT_PROOF_LIMIT_SECS, Stream, Until};

pub use sva_engine::{Answer, Extent, Label, Output, Representation};
pub use sva_engine::{Cache, CachePolicy, PrunePolicy};

pub const ROOT: &str = "master";
pub const PROBE: &str = "probe";

pub struct Rendered {
    pub config: RenderConfig,
    pub expression: String,
    pub target: String,
    pub render: sva_engine::Render,
    /// So a structural check needs no second parse, and an ad-hoc target is the same `probe`.
    pub graph: Graph,
}

impl Rendered {
    pub fn answer(&self, node: &str, representation: Representation) -> Result<Answer, CliError> {
        let written = |e| CliError::Engine(as_written(e, &self.expression));
        let id = self.render.node(node).map_err(written)?;
        let mut answer = sva_engine::answer(&self.render, id, representation).map_err(written)?;
        self.attribute(&mut answer)?;
        Ok(answer)
    }

    /// An alias score is only readable beside what else moves with the rate: a sampled loop
    /// is a different signal at the oversampled rate, and the engine cannot see the graph.
    fn attribute(&self, answer: &mut Answer) -> Result<(), CliError> {
        let Output::Alias(alias) = &mut answer.value else {
            return Ok(());
        };
        alias.rate_dependent = sva_engine::rate_dependent(&self.graph, &self.target)
            .map_err(CliError::Engine)?
            .len();
        alias.instances = self.render.tys.paths().count();
        Ok(())
    }

    pub fn label(&self) -> Option<&sva_engine::Label> {
        self.render.labels.get(&self.render.root)
    }
}

/// One render: `target` an expression over the composition `source` holds, its own ref
/// read over an interval where it writes one.
pub struct Job<'a> {
    pub source: &'a dyn Source,
    pub target: &'a str,
    /// `None` ends the render where the interval or the target's support ends.
    pub until: Option<&'a str>,
    pub rate: Option<u32>,
    pub bits: Option<i32>,
    pub cache: Option<&'a Cache>,
    pub cache_policy: Option<CachePolicy>,
    pub asked: &'a [Asked],
    /// The operation count the caller acknowledges paying; the profile's own where `None`.
    pub flop_budget: Option<u128>,
    pub volatile: &'a [String],
}

impl<'a> Job<'a> {
    pub fn over(source: &'a dyn Source, target: &'a str) -> Job<'a> {
        Job {
            source,
            target,
            until: None,
            rate: None,
            bits: None,
            cache: None,
            cache_policy: None,
            asked: &[],
            flop_budget: None,
            volatile: &[],
        }
    }
}

fn settle(job: &Job) -> Result<(Graph, RenderConfig), CliError> {
    let Target { expr, interval } = target(job.target)?;
    let mut graph = settled(sva_ast::load_reaching(
        job.source,
        &roots_of(job.source, &expr)?
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    ))?;
    let parsed = sva_ast::parse_expr(&expr).map_err(|d| {
        CliError::BadProbe(format!(
            "`{}` does not parse as an expression: {}",
            job.target, d.message
        ))
    })?;
    define_probe(&mut graph, parsed)?;
    let rate = job.rate.unwrap_or(DEFAULT_SAMPLE_RATE);
    let per_bar = graph.seconds_per_bar();
    let range = match interval {
        None => Range::default(),
        Some((start, end)) => Range {
            start: start.sample(rate, per_bar)?,
            end: end.sample(rate, per_bar)?,
        },
    };
    if let Range {
        start: Some(start),
        end: Some(end),
    } = range
        && end <= start
    {
        return Err(CliError::Usage(format!(
            "`{}` reads an interval from sample {start} to {end}, which holds none",
            job.target
        )));
    }
    let until = match job.until {
        Some(text) => Some(until(text, rate, per_bar)?),
        None => None,
    };
    let mut config = RenderConfig {
        range,
        until,
        ..RenderConfig::at(rate)
    };
    if let Some(budget) = job.flop_budget {
        config.flop_budget = budget;
    }
    if let Some(bits) = job.bits {
        config.profile.precision_bits = precision(bits)?;
    }
    config.volatile = job.volatile.to_vec();
    config.cache_policy = job.cache_policy;
    config.asks = job
        .asked
        .iter()
        .map(|asked| Ask {
            node: asked.node.clone().unwrap_or_else(|| PROBE.to_string()),
            representation: asked.representation,
        })
        .collect();
    Ok((graph, config))
}

/// `trace` names one instance of a parameterized file as `<path>(<name>=<value>, ..)`.
fn instance_call(text: &str) -> Option<(&str, &str)> {
    let (path, rest) = text.split_once('(')?;
    let binds = rest.strip_suffix(')')?.trim();
    (!binds.is_empty() && all_named(binds)).then_some((path, binds))
}

/// Only commas and equals outside a bind's own parens count: a bind's value may be a call.
fn all_named(binds: &str) -> bool {
    let mut depth = 0i32;
    let mut named = false;
    for c in binds.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return false,
            ')' => depth -= 1,
            '=' if depth == 0 => named = true,
            ',' if depth == 0 && !std::mem::take(&mut named) => return false,
            _ => {}
        }
    }
    named && depth == 0
}

/// The ref an instance name stands for, so `lint` and `trace` answer for the same node.
fn instance_read(graph: &Graph, text: &str) -> Option<sva_ast::Expr> {
    let (path, binds) = instance_call(text)?;
    if !graph.defines(path) {
        return None;
    }
    sva_ast::parse_expr(&format!("@{path}(t, {binds})")).ok()
}

pub fn execute(job: Job) -> Result<Rendered, CliError> {
    let (graph, config) = settle(&job)?;
    let render = render(&graph, PROBE, config, job.cache)
        .map_err(|e| CliError::Engine(as_written(e, job.target)))?;
    Ok(Rendered {
        config: render.config.clone(),
        expression: job.target.to_string(),
        target: PROBE.to_string(),
        render,
        graph,
    })
}

pub fn stream(job: &Job, block: usize, bindings: &[(String, f64)]) -> Result<Stream, CliError> {
    let (graph, config) = settle(job)?;
    let target = graph
        .expr(PROBE)
        .cloned()
        .expect("the target was defined as the probe");
    let config = StreamConfig {
        rate: config.rate,
        block,
        range: config.range,
        until: config.until,
    };
    Stream::open(&graph, &target, bindings, config)
        .map_err(|e| CliError::Engine(as_written(e, job.target)))
}

/// A double holds no bit past its own mantissa, and one bit writes only zero.
fn precision(bits: i32) -> Result<i32, CliError> {
    match (2..=52).contains(&bits) {
        true => Ok(bits),
        false => Err(CliError::Usage(format!(
            "`--bits` takes a precision from 2 to 52 bits, not {bits}"
        ))),
    }
}

/// The engine knows a target only as the node it was defined as, so a refusal that names
/// that node names the target as the caller wrote it instead.
fn as_written(refused: EngineError, target: &str) -> EngineError {
    match refused {
        EngineError::Refused(mut d) => {
            if d.location.node == PROBE {
                d.location.node = target.to_string();
            }
            d.message = d
                .message
                .replace(&format!("`{PROBE}`"), &format!("`{target}`"));
            EngineError::Refused(d)
        }
        EngineError::Binding { node, span, fault } if node == PROBE => EngineError::Binding {
            node: target.to_string(),
            span,
            fault,
        },
        other => other,
    }
}

pub fn probe(dir: &Path, expression: &str) -> Result<Rendered, CliError> {
    execute(Job::over(&Dir::at(dir), expression))
}

pub fn cwd() -> Result<std::path::PathBuf, CliError> {
    std::env::current_dir()
        .map_err(|e| CliError::Io(format!("could not read the current directory: {e}")))
}

pub fn prepared(source: &dyn Source) -> Result<Graph, CliError> {
    settled(sva_ast::load(source))
}

/// Argv math: `trace` and `lint` each refuse a target no node answers for here, under one
/// code and one message.
pub fn define_probe_for(graph: &mut Graph, text: &str) -> Result<(), CliError> {
    let expr = match instance_read(graph, text) {
        Some(read) => read,
        None if names_a_missing_node(text) => {
            return Err(CliError::NotFound(format!(
                "`{text}` is not a node this composition defines"
            )));
        }
        None => sva_ast::parse_expr(text).map_err(|d| {
            CliError::BadProbe(format!(
                "`{text}` is not a node in this composition, and does not parse as an \
                 expression: {}",
                d.message
            ))
        })?,
    };
    define_probe(graph, expr)
}

/// Only a path, or instance syntax, is a node; the rest is math the parser and engine answer
/// for. `t` and `A4` are spelled like paths and read as values.
fn names_a_missing_node(text: &str) -> bool {
    match instance_call(text) {
        Some((path, _)) => sva_ast::names_a_node(path),
        None => !text.contains('(') && sva_ast::names_a_node(text) && !reads_as_a_value(text),
    }
}

fn reads_as_a_value(text: &str) -> bool {
    match sva_ast::parse_expr(text) {
        Ok(sva_ast::Expr::Lit(_)) => true,
        Ok(sva_ast::Expr::Var(name)) => sva_engine::instantiate::is_reserved(&name),
        _ => false,
    }
}

/// Argv math settled the way a file is; `concat` is no file-only dialect.
fn define_probe(graph: &mut Graph, expr: sva_ast::Expr) -> Result<(), CliError> {
    let defined = graph
        .define_arranged(PROBE, expr)
        .map_err(|r| CliError::Refusals(vec![r]))?;
    if !defined {
        return Err(CliError::BadProbe(format!(
            "this composition already has a node named `{PROBE}`"
        )));
    }
    tempo::refuse_unresolved_bars(graph)
}

pub fn settled(loaded: Result<Graph, Vec<Refusal>>) -> Result<Graph, CliError> {
    let mut graph = loaded.map_err(CliError::Refusals)?;
    tempo::resolve(&mut graph)?;
    graph.desugar_arrangement().map_err(CliError::Refusals)?;
    Ok(graph)
}

/// Read by name rather than through a ref.
pub const RESERVED_VARIABLES: [&str; 3] = ["bpm", "meter", "key"];

/// A path the source answers for is a node; anything else is math, and its reads are roots.
pub fn roots_of(source: &dyn Source, target: &str) -> Result<Vec<String>, CliError> {
    let mut roots: Vec<String> = RESERVED_VARIABLES
        .iter()
        .flat_map(|n| [(*n).to_string(), format!("{}/{n}", sva_ast::VARIABLES)])
        .collect();
    match target {
        target if source.get(target).map_err(CliError::Io)?.is_some() => {
            roots.push(target.to_string());
        }
        text => match instance_call(text).map(|(path, _)| path) {
            Some(path) if source.get(path).map_err(CliError::Io)?.is_some() => {
                roots.push(path.to_string());
            }
            _ => {
                if let Ok(expr) = sva_ast::parse_expr(text) {
                    roots.extend(sva_ast::reads_of(PROBE, &expr));
                }
            }
        },
    }
    Ok(roots)
}
