// Concern: advises a crop where an uncropped node is proven quiet long before it ends | Non-concern: the bound (sva-engine), cropping anything | IO: (&Graph, roots, Quiet) -> Vec<Finding>

use std::collections::{BTreeMap, BTreeSet};

use sva_ast::{BinOp, Expr, Graph, Source, SpanUnit, TokenKind};
use sva_core::{CliError, LintCode, Severity, Tempo};
use sva_engine::{DEFAULT_PROOF_LIMIT_SECS, DEFAULT_SAMPLE_RATE, QuietTail, RenderConfig};

use crate::lint::Finding;

pub const DEFAULT_QUIET_FLOOR_DB: f64 = -96.0;
pub const DEFAULT_QUIET_AFTER_SECS: f64 = 1.0;

/// The level a tail must stay under, and how long past that a render must still run it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quiet {
    pub floor_db: f64,
    pub after: f64,
    pub after_unit: SpanUnit,
}

impl Default for Quiet {
    fn default() -> Quiet {
        Quiet {
            floor_db: DEFAULT_QUIET_FLOOR_DB,
            after: DEFAULT_QUIET_AFTER_SECS,
            after_unit: SpanUnit::Seconds,
        }
    }
}

impl Quiet {
    pub const KEYS: [&'static str; 2] = ["quiet_floor", "quiet_after"];

    pub fn set(&mut self, key: &str, raw: &str) -> Result<(), CliError> {
        let refused = |what: &str| CliError::Usage(format!("`{key}` needs {what}, got `{raw}`"));
        match key {
            "quiet_floor" => {
                let digits = raw.strip_suffix("db").unwrap_or(raw);
                self.floor_db = match digits.parse::<f64>() {
                    Ok(db) if db.is_finite() && db < 0.0 => db,
                    _ => return Err(refused("a level under 0 dB, as `-96` or `-96db`")),
                };
            }
            "quiet_after" => {
                (self.after, self.after_unit) = match sva_ast::tokenize(raw).as_deref() {
                    Ok([one]) => match one.kind {
                        TokenKind::Num(v) => (v, SpanUnit::Seconds),
                        TokenKind::Time(v, unit) => (v, unit),
                        _ => return Err(refused("a time, as `2s`, `500ms` or `1b`")),
                    },
                    _ => return Err(refused("a time, as `2s`, `500ms` or `1b`")),
                };
                if !(self.after >= 0.0 && self.after.is_finite()) {
                    return Err(refused("a time at or above zero"));
                }
            }
            other => {
                return Err(CliError::Usage(format!(
                    "`{other}` is no lint setting; the settings are {}",
                    Quiet::KEYS.join(", ")
                )));
            }
        }
        Ok(())
    }
}

/// One advisory per file every instance of which is proven quiet: a crop at the latest of
/// their instants cuts nothing louder than the floor from any of them.
pub fn quiet_findings(
    source: &dyn Source,
    graph: &Graph,
    roots: &[String],
    quiet: &Quiet,
    tempo: Option<Tempo>,
) -> Result<Vec<Finding>, CliError> {
    let after = match (quiet.after_unit, tempo) {
        (SpanUnit::Seconds, _) => quiet.after,
        (SpanUnit::Bars, Some(tempo)) => quiet.after * tempo.seconds_per_bar,
        (SpanUnit::Bars, None) => {
            return Err(CliError::Usage(format!(
                "`quiet_after={}b` is in bars, and this composition declares no bpm/meter",
                quiet.after
            )));
        }
    };
    let level = 10f64.powf(quiet.floor_db / 20.0);
    let (instances, unsure) = every_instance(source, graph, roots, level);
    let mut files: BTreeMap<&str, Vec<&QuietTail>> = BTreeMap::new();
    for tail in instances.values() {
        files.entry(tail.file.as_str()).or_default().push(tail);
    }
    let mut out = Vec::new();
    for (file, held) in files {
        let proven: Option<Vec<f64>> = held.iter().map(|t| t.quiet_from).collect();
        let Some(from) = proven.and_then(|p| p.into_iter().reduce(f64::max)) else {
            continue;
        };
        let open = held.iter().filter(|t| t.support.1 == f64::INFINITY).count();
        // An end the node's own crop writes is no run-on; one with no end runs as far as computed.
        let cropped = graph.expr(file).is_some_and(crops);
        let run_on = held
            .iter()
            .map(|t| match (t.support.1.is_finite(), cropped) {
                (false, _) => t.computed_until.unwrap_or(f64::NEG_INFINITY),
                (true, false) => t.support.1,
                (true, true) => f64::NEG_INFINITY,
            })
            .map(|end| end - from)
            .fold(f64::NEG_INFINITY, f64::max);
        // Before 0 a crop from 0 would cut what the instance holds there.
        let before = held.iter().any(|t| t.support.0 < 0.0);
        if before || unsure.contains(file) || run_on < after || run_on <= 0.0 {
            continue;
        }
        let Some(body) = body(source, graph, file) else {
            continue;
        };
        let at = spelled(from, tempo, f64::ceil);
        let computed = match run_on.is_finite() {
            true => format!("for {} past that", spelled(run_on, tempo, f64::floor)),
            false => "with no end".to_string(),
        };
        let count = match (open, held.len()) {
            (_, 1) => String::new(),
            (n, all) if n == all || n == 0 => format!(" in each of its {all} instances"),
            (n, all) => format!(" in all {all} of its instances, {n} of them with no end"),
        };
        out.push(Finding {
            code: LintCode::QuietTail,
            severity: Severity::Advice,
            subject: file.to_string(),
            message: format!(
                "`{file}` has no crop that ends it, and its tail bound is under {} dB from {at} \
                 on{count}, yet it is computed {computed}: `crop({body}, 0s, {at})`",
                quiet.floor_db
            ),
            line: None,
        });
    }
    Ok(out)
}

/// Every instance any root reaches, each computed as far as the furthest root computes it,
/// and every file a root reaches that could not be read, whose instances are then unknown.
fn every_instance(
    source: &dyn Source,
    graph: &Graph,
    roots: &[String],
    level: f64,
) -> (BTreeMap<String, QuietTail>, BTreeSet<String>) {
    let mut instances: BTreeMap<String, QuietTail> = BTreeMap::new();
    let mut unsure = BTreeSet::new();
    for root in roots.iter().collect::<BTreeSet<_>>() {
        let config = RenderConfig::at(DEFAULT_SAMPLE_RATE);
        let got = sva_engine::quiet_tails(graph, root, config, level, DEFAULT_PROOF_LIMIT_SECS);
        let Ok(found) = got else {
            match sva_ast::load_reaching(source, &[root.as_str()]) {
                Ok(reached) => unsure.extend(reached.paths().map(str::to_string)),
                Err(_) => unsure.extend(graph.paths().map(str::to_string)),
            }
            continue;
        };
        for tail in found {
            match instances.get_mut(&tail.instance) {
                Some(held) => held.computed_until = later(held.computed_until, tail.computed_until),
                None => {
                    instances.insert(tail.instance.clone(), tail);
                }
            }
        }
    }
    (instances, unsure)
}

fn later(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// Whether a written crop ends the whole value: a sum only where every addend is cropped.
fn crops(e: &Expr) -> bool {
    match e {
        Expr::Call { name, .. } => name == "crop",
        Expr::Bin(BinOp::Mul, l, r) => crops(l) || crops(r),
        Expr::Bin(BinOp::Div, l, _) => crops(l),
        Expr::Bin(BinOp::Add | BinOp::Sub, l, r) => crops(l) && crops(r),
        _ => false,
    }
}

/// To the thousandth in the composition's own unit, rounded the way that keeps a crop safe.
fn spelled(secs: f64, tempo: Option<Tempo>, round: fn(f64) -> f64) -> String {
    let (amount, unit) = match tempo {
        Some(tempo) => (secs / tempo.seconds_per_bar, "b"),
        None => (secs, "s"),
    };
    format!("{}{unit}", round(amount * 1000.0) / 1000.0)
}

/// The expression a node file holds, as written; a grid holds rows, not one expression.
fn body(source: &dyn Source, graph: &Graph, file: &str) -> Option<String> {
    if graph.grid(file).is_some() {
        return None;
    }
    let text = source.get(file).ok()??;
    let lines: Vec<&str> = text
        .lines()
        .map(|l| sva_ast::strip_line_comment(l).trim())
        .filter(|l| !l.is_empty())
        .collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}
