// Concern: the parse->tempo->render pipeline shared by both front ends | Non-concern: argv (sva-cli), JS bindings (sva-wasm) | IO: (a Source, a target) -> Rendered, a Stream or CliError

mod answer;
mod builtins;
mod cli_error;
mod duration;
pub mod json;
mod lint_code;
mod outline;
mod output;
mod query;
mod tempo;

pub use answer::{
    CacheReport, Report, SAMPLE_LIMIT, answer_json, label_json, query_data, stats_json, value_json,
};
pub use builtins::{Builtins, Callable, Crossing, builtins, builtins_data};
pub use cli_error::{CliError, LintViolation, lint_diagnostic};
pub use lint_code::LintCode;
pub use outline::outline_data;
pub use output::{Diagnostic, Severity, diagnostics_json, error_envelope, success_envelope};
pub use query::{
    Asked, DEFAULT_LEDGER_DEPTH, DEFAULT_MAX_PEAKS, DEFAULT_OVERSAMPLE, DEFAULT_SILENT_BITS,
    DEFAULT_SILENT_MAX_SECS, REPRESENTATIONS, RETIRED, Shaping, WindowEdge, is_wav,
    representation_for, retired, silence, silent_edge, window_edge, window_for,
};
pub use tempo::refuse_unresolved_bars;

use std::path::Path;

use sva_ast::{Dir, Graph, Refusal, Source, SpanUnit};
use sva_engine::{
    Ask, BindingFault, Cache, DEFAULT_SAMPLE_RATE, EngineError, PSYCHOACOUSTIC_V1, Render,
    RenderConfig, StreamConfig, render_until_silent, render_with_slots,
};

pub use sva_engine::{Checkpoint, Silent, Stream};

pub use sva_engine::{Answer, Horizon, Label, Output, Representation};
pub use sva_engine::{DiskCache, MemoryCache, Slots};

pub const ROOT: &str = "master";
pub const PROBE: &str = "probe";

pub const DEFAULT_SECONDS: f64 = 1.0;

pub struct Rendered {
    pub config: RenderConfig,
    pub target: String,
    pub render: Render,
    /// So a structural check needs no second parse, and an ad-hoc target is the same `probe`.
    pub graph: Graph,
}

impl Rendered {
    pub fn answer(&self, node: &str, representation: Representation) -> Result<Answer, CliError> {
        let id = self.render.node(node).map_err(CliError::Engine)?;
        let mut answer =
            sva_engine::answer(&self.render, id, representation).map_err(CliError::Engine)?;
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

/// A `target` the composition holds is that node; anything else is argv math.
pub struct Job<'a> {
    pub source: &'a dyn Source,
    pub target: Option<&'a str>,
    pub from: Option<WindowEdge>,
    pub until: Option<WindowEdge>,
    pub sample_rate: Option<u32>,
    pub cache: Option<&'a dyn Cache>,
    /// Only what the target reaches, so a node nothing reaches is never read or refused.
    pub reaching: bool,
    /// The instance every reading is taken of; the target itself where this is `None`.
    pub reading: Option<&'a str>,
    pub representations: Vec<Representation>,
    /// The operation count the caller acknowledges paying; the profile's own where `None`.
    pub flop_budget: Option<u128>,
    pub volatile: &'a [String],
    pub slots: Option<&'a Slots>,
    /// Render until silence is proven, in place of `until`.
    pub silent: Option<Silent>,
}

impl<'a> Job<'a> {
    pub fn over(source: &'a dyn Source) -> Job<'a> {
        Job {
            source,
            target: None,
            from: None,
            until: None,
            sample_rate: None,
            cache: None,
            reaching: false,
            reading: None,
            representations: Vec::new(),
            flop_budget: None,
            volatile: &[],
            slots: None,
            silent: None,
        }
    }
}

fn settle(job: &Job, asked: Option<&str>) -> Result<(Graph, String, RenderConfig), CliError> {
    let mut graph = match job.reaching {
        false => prepared(job.source)?,
        true => settled(sva_ast::load_reaching(
            job.source,
            &roots_of(job.source, asked)?
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ))?,
    };
    let target = match asked {
        None => ROOT.to_string(),
        Some(name) if graph.defines(name) => name.to_string(),
        Some(text) => {
            define_probe_for(&mut graph, text)?;
            PROBE.to_string()
        }
    };
    let until = match job.silent {
        Some(silent) => Some(WindowEdge::Secs(silent.max_secs)),
        None => job.until,
    };
    let mut config = config_for(&graph, &target, job.from, until, job.sample_rate)?;
    if let Some(budget) = job.flop_budget {
        config.flop_budget = budget;
    }
    config.volatile = job.volatile.to_vec();
    let node = job.reading.unwrap_or(&target);
    config.asks = job
        .representations
        .iter()
        .map(|representation| Ask {
            node: node.to_string(),
            representation: *representation,
        })
        .collect();
    Ok((graph, target, config))
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

/// The ref an instance name stands for, so `render` and `trace` answer for the same node.
fn instance_read(graph: &Graph, text: &str) -> Option<sva_ast::Expr> {
    let (path, binds) = instance_call(text)?;
    if !graph.defines(path) {
        return None;
    }
    sva_ast::parse_expr(&format!("@{path}(t, {binds})")).ok()
}

pub fn execute(job: Job) -> Result<Rendered, CliError> {
    let cache = job.cache;
    let rendered = rendered(job);
    if let Some(store) = cache {
        store.sweep();
    }
    rendered
}

fn rendered(job: Job) -> Result<Rendered, CliError> {
    let (graph, target, config) = settle(&job, job.target)?;
    let refused = match rendering(&job, &graph, &target, config) {
        Ok(render) => {
            return Ok(Rendered {
                config: render.config.clone(),
                render,
                target,
                graph,
            });
        }
        Err(refused) => refused,
    };
    match instances_behind(job.source, &target, &refused) {
        Some(held) if held.len() == 1 => at_instance(&job, &held[0]),
        Some(held) => Err(CliError::Engine(EngineError::AmbiguousNode(target, held))),
        None => Err(CliError::Engine(refused)),
    }
}

pub fn instances_behind(
    source: &dyn Source,
    target: &str,
    refused: &EngineError,
) -> Option<Vec<String>> {
    unbound(refused)
        .then(|| instances_of(source, target))
        .flatten()
}

fn unbound(refused: &EngineError) -> bool {
    matches!(
        refused,
        EngineError::Binding {
            fault: BindingFault::Unbound(..),
            ..
        }
    )
}

/// The instances a whole composition expanded a file into; a render reaches none of them.
fn instances_of(source: &dyn Source, target: &str) -> Option<Vec<String>> {
    let graph = prepared(source).ok()?;
    let (instances, _) = sva_engine::instantiate::from_roots(&graph, &[ROOT.to_string()]).ok()?;
    let held: Vec<String> = instances.instances_of(target).collect();
    (!held.is_empty()).then_some(held)
}

/// One instance is the node the caller meant, read as if they had named it themselves.
fn at_instance(job: &Job, instance: &str) -> Result<Rendered, CliError> {
    let (graph, target, config) = settle(job, Some(instance))?;
    let render = rendering(job, &graph, &target, config).map_err(CliError::Engine)?;
    Ok(Rendered {
        config: render.config.clone(),
        render,
        target,
        graph,
    })
}

/// The horizon the caller named, or the one silence ends.
fn rendering(
    job: &Job,
    graph: &Graph,
    target: &str,
    config: RenderConfig,
) -> Result<Render, EngineError> {
    match job.silent {
        Some(silent) => render_until_silent(graph, target, config, silent, job.cache, job.slots),
        None => render_with_slots(graph, target, config, job.cache, job.slots),
    }
}

/// `job`'s target settled as a render of it is, each of `bindings` a named argument on it.
pub fn stream(job: &Job, block: usize, bindings: &[(String, f64)]) -> Result<Stream, CliError> {
    let (graph, target, config) = settle(job, job.target)?;
    let config = StreamConfig {
        rate: config.rate,
        block,
        silent: job.silent,
    };
    Stream::open(&graph, &target, bindings, config).map_err(CliError::Engine)
}

pub fn run(dir: &Path) -> Result<Rendered, CliError> {
    execute(Job::over(&Dir::at(dir)))
}

pub fn probe(dir: &Path, expression: &str) -> Result<Rendered, CliError> {
    execute(Job {
        target: Some(expression),
        ..Job::over(&Dir::at(dir))
    })
}

pub fn cwd() -> Result<std::path::PathBuf, CliError> {
    std::env::current_dir()
        .map_err(|e| CliError::Io(format!("could not read the current directory: {e}")))
}

pub fn prepared(source: &dyn Source) -> Result<Graph, CliError> {
    settled(sva_ast::load(source))
}

/// Argv math: `render`, `trace` and `lint` each refuse a target no node answers for here,
/// under one code and one message.
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
pub fn roots_of(source: &dyn Source, target: Option<&str>) -> Result<Vec<String>, CliError> {
    let mut roots: Vec<String> = RESERVED_VARIABLES
        .iter()
        .flat_map(|n| [(*n).to_string(), format!("{}/{n}", sva_ast::VARIABLES)])
        .collect();
    match target {
        None => roots.push(ROOT.to_string()),
        Some(target) if source.get(target).map_err(CliError::Io)?.is_some() => {
            roots.push(target.to_string());
        }
        Some(text) => match instance_call(text).map(|(path, _)| path) {
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

/// The horizon a reading runs against: the window the caller named, else the node's own
/// stated extent, reaching back to whatever a `crop` declares before zero.
pub fn config_for(
    graph: &Graph,
    root: &str,
    from: Option<WindowEdge>,
    until: Option<WindowEdge>,
    sample_rate: Option<u32>,
) -> Result<RenderConfig, CliError> {
    let declared = duration::widest_crop(graph, root);
    let length = match graph.span(root) {
        Some(span) if span.unit == SpanUnit::Seconds => Some(span.amount),
        Some(span) => {
            return Err(CliError::BadTempo(format!(
                "`{root}`'s span is still {} bars, unresolved; give bpm/meter, or a seconds span",
                span.amount
            )));
        }
        None => declared.end,
    };
    let asked = window_for(from, until, graph.seconds_per_bar())?;
    let end = match asked.end_secs.is_finite() {
        true => asked.end_secs,
        false => length.unwrap_or(DEFAULT_SECONDS),
    };
    let stated = !matches!(from, None | Some(WindowEdge::End));
    let start = match stated {
        true => asked.start_secs,
        false => declared.start.unwrap_or(0.0).min(0.0),
    };
    if end <= start {
        return Err(CliError::Usage(format!(
            "a reading runs from {start}s to {end}s, which is no window; give --to past --from"
        )));
    }
    Ok(RenderConfig {
        rate: sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE),
        horizon: Horizon::secs(start, end),
        profile: PSYCHOACOUSTIC_V1,
        asks: Vec::new(),
        flop_budget: PSYCHOACOUSTIC_V1.flop_budget,
        volatile: Vec::new(),
    })
}
