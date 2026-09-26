// Concern: what a composition can be told about itself without rendering a sample | Non-concern: rendering, or judging how it sounds | IO: (dir[, target]) -> Vec<Finding> or CliError

use std::collections::BTreeSet;
use std::path::Path;

use sva_ast::{Binds, Expr, Graph, Skip, Source, children, ref_spans, resolve_ref_path};
use sva_ast::{MAX_TAG_CHARS, MAX_TAGS, is_plain_tag, parse_doc_comment};
use sva_core::{
    CliError, Diagnostic, LintCode, LintViolation, ROOT, Severity, lint_diagnostic, prepared,
    refuse_unresolved_bars, settled,
};
use sva_engine::EngineError;

/// These ride a `success` envelope; a refusal raises [`sva_core::CliError::LintRefused`].
pub struct Finding {
    pub code: LintCode,
    pub severity: Severity,
    pub subject: String,
    pub message: String,
    /// The 1-based line the check found it on, where it knows one.
    pub line: Option<usize>,
}

impl Finding {
    pub fn diagnostic(&self) -> Diagnostic {
        lint_diagnostic(
            self.code,
            &self.subject,
            &self.message,
            self.severity,
            self.line,
        )
    }
}

pub struct LintReport {
    pub nodes: usize,
    pub findings: Vec<Finding>,
}

/// With no target, lints the whole directory against `master`; with one, lints only what it
/// reaches, against it instead.
pub fn lint(dir: &Path, target: Option<&str>) -> Result<LintReport, CliError> {
    let source = sva_ast::Dir::at(dir);
    match target {
        None => lint_whole(&source),
        Some(target) => lint_reaching(&source, target),
    }
}

/// A note or a rendering may sit beside a composition; the walk says which it passed over,
/// less the documents. A dot directory never reaches here: `Dir::paths` skips one.
fn not_nodes(graph: &Graph) -> Vec<Finding> {
    graph
        .skipped()
        .iter()
        .filter(|s| s.reason != Skip::Unnameable || !is_document(&s.path))
        .map(|s| {
            let path = &s.path;
            let (code, message) = match s.reason {
                Skip::Unnameable => (
                    LintCode::NotANode,
                    format!("no ref can name `{path}`, so it is not read as a node"),
                ),
                Skip::Special => (
                    LintCode::NotAFile,
                    format!("`{path}` is a socket, FIFO or device, so no text was read from it"),
                ),
            };
            Finding {
                code,
                severity: Severity::Advice,
                subject: path.clone(),
                message,
                line: None,
            }
        })
        .collect()
}

/// Prose and a reference reading sit beside a composition's nodes, under any casing.
fn is_document(path: &str) -> bool {
    match path.rsplit_once('.') {
        Some((_, suffix)) => matches!(suffix.to_ascii_lowercase().as_str(), "md" | "json"),
        None => false,
    }
}

fn lint_whole(source: &dyn Source) -> Result<LintReport, CliError> {
    let graph = prepared(source)?;
    refuse_unresolved_bars(&graph)?;
    let violations = lint_violations(source, &graph);
    // A lint violation refuses before a structural one.
    if violations.is_empty() && graph.defines(ROOT) {
        sva_engine::check_structure(&graph, ROOT).map_err(CliError::Engine)?;
    }

    let mut findings = entry_typing(&graph);
    if !graph.defines(ROOT) {
        findings.push(Finding {
            code: LintCode::NoDefaultRoot,
            severity: Severity::Advice,
            subject: ROOT.to_string(),
            message: format!(
                "no `{ROOT}` file, so a render must name the node it wants — every node is \
                 still a root"
            ),
            line: None,
        });
    }
    findings.extend(not_nodes(&graph));
    findings.extend(unreached(&graph));
    findings.extend(grid_row_counts(&graph));
    findings.extend(tag_findings(source, &graph));
    findings.extend(crate::variables::key_findings(&graph));
    findings.extend(crate::rates::rate_findings(&graph));
    findings.extend(crate::windows::window_findings(
        &graph,
        &whole_roots(&graph),
    ));
    verdict(graph.paths().count(), violations, findings)
}

/// The status is the verdict; `data.diagnostics[]` carries everything the scan found.
fn verdict(
    nodes: usize,
    violations: Vec<LintViolation>,
    findings: Vec<Finding>,
) -> Result<LintReport, CliError> {
    if violations.is_empty() {
        return Ok(LintReport { nodes, findings });
    }
    let mut every = violations;
    every.extend(findings.into_iter().map(|f| LintViolation {
        code: f.code,
        severity: f.severity,
        subject: f.subject,
        message: f.message,
        line: f.line,
    }));
    Err(CliError::LintRefused(every))
}

fn whole_roots(graph: &Graph) -> Vec<String> {
    let mut out: Vec<String> = graph
        .defines(ROOT)
        .then(|| ROOT.to_string())
        .into_iter()
        .collect();
    out.extend(entry_points(graph));
    out
}

/// Every node nothing references is a root of its own, and a whole-directory lint says what
/// typing each one found. One template nothing binds is not a broken directory, so this
/// reports rather than stopping the pass; `master`, which a bare render targets, refuses.
fn entry_typing(graph: &Graph) -> Vec<Finding> {
    entry_points(graph)
        .into_iter()
        .filter(|p| p != ROOT)
        .filter_map(|path| {
            let refused = sva_engine::check_structure(graph, &path).err()?;
            Some(Finding {
                code: LintCode::EntryPointRefused,
                severity: Severity::Warning,
                subject: path,
                message: refused.to_string(),
                line: None,
            })
        })
        .collect()
}

fn lint_reaching(source: &dyn Source, target: &str) -> Result<LintReport, CliError> {
    let held = sva_core::roots_of(source, Some(target))?;
    let roots: Vec<&str> = held.iter().map(String::as_str).collect();
    let mut graph = settled(sva_ast::load_reaching(source, &roots))?;
    refuse_unresolved_bars(&graph)?;
    let violations = lint_violations(source, &graph);
    // A target nothing defines is the same `probe` a render builds, so a typo refuses here.
    let root = match graph.defines(target) {
        true => target.to_string(),
        false => {
            sva_core::define_probe_for(&mut graph, target)?;
            sva_core::PROBE.to_string()
        }
    };
    if let Err(refused) = sva_engine::check_structure(&graph, &root)
        && violations.is_empty()
    {
        // A closure loaded from the target alone never sees the call sites that bind it.
        return match sva_core::instances_behind(source, target, &refused).as_deref() {
            Some([only]) if only != target => lint_reaching(source, only),
            Some(held) => Err(CliError::Engine(EngineError::AmbiguousNode(
                target.to_string(),
                held.to_vec(),
            ))),
            None => Err(CliError::Engine(refused)),
        };
    }
    // No `unreached` here (every node is reached by construction); the other checks are per-node.
    let mut findings = grid_row_counts(&graph);
    findings.extend(tag_findings(source, &graph));
    findings.extend(crate::variables::key_findings(&graph));
    findings.extend(crate::rates::rate_findings(&graph));
    findings.extend(crate::windows::window_findings(&graph, &[root]));
    verdict(graph.paths().count(), violations, findings)
}

/// A row count tiles a span as a subdivision of a bar or as whole bars; both count.
fn grid_row_counts(graph: &Graph) -> Vec<Finding> {
    let mut findings = Vec::new();
    for path in graph.paths() {
        let Some(grid) = graph.grid(path) else {
            continue;
        };
        let Some(bars) = grid.bar_span.filter(|b| *b > 0.0) else {
            continue;
        };
        let rows_per_bar = grid.row_count as f64 / bars;
        let bars_per_row = bars / grid.row_count as f64;
        let whole = |v: f64| (v - v.round()).abs() <= 1e-9;
        if !whole(rows_per_bar) && !whole(bars_per_row) {
            findings.push(Finding {
                code: LintCode::GridRowsPerBar,
                severity: Severity::Warning,
                subject: path.to_string(),
                message: format!(
                    "`{path}` has {} rows over {bars} bar(s), {rows_per_bar:.4} rows/bar and \
                     {bars_per_row:.4} bars/row — neither a whole subdivision of a bar nor a \
                     whole number of bars a row, likely a stray or missing row",
                    grid.row_count
                ),
                line: None,
            });
        }
    }
    findings
}

/// Refuses with every violation found, not just the first: a tree-wide scan, unlike
/// `check_arity`/`check_feedback`'s single graph evaluation.
fn lint_violations(source: &dyn Source, graph: &Graph) -> Vec<LintViolation> {
    let mut violations = Vec::new();
    violations.extend(doc_comment_violations(source, graph));
    violations.extend(long_comment_block_violations(source, graph));
    violations.extend(long_expression_body_violations(source, graph));
    violations
}

/// Even a justified multi-line block stays under this.
const LONG_COMMENT_BLOCK_CHARS: usize = 1000;

/// The line-1 doc-comment's own run is excluded from the count.
fn long_comment_block_violations(source: &dyn Source, graph: &Graph) -> Vec<LintViolation> {
    let mut violations = Vec::new();
    for path in graph.paths() {
        let Ok(Some(text)) = source.get(path) else {
            continue;
        };
        // start line, running char total, first line's own char length.
        let mut run: Option<(usize, usize, usize)> = None;
        for (idx, line) in text.lines().enumerate() {
            if line.trim_start().starts_with(';') {
                let line_chars = line.chars().count();
                run = Some(match run {
                    Some((start, chars, first)) => (start, chars + line_chars, first),
                    None => (idx + 1, line_chars, line_chars),
                });
            } else {
                flush_comment_run(run.take(), path, &mut violations);
            }
        }
        flush_comment_run(run, path, &mut violations);
    }
    violations
}

/// Drops the header line's own char count (not a flat `1`) when the run starts at line 1.
fn flush_comment_run(
    run: Option<(usize, usize, usize)>,
    path: &str,
    violations: &mut Vec<LintViolation>,
) {
    let Some((start, chars, first)) = run else {
        return;
    };
    let chars = if start == 1 {
        chars.saturating_sub(first)
    } else {
        chars
    };
    if chars > LONG_COMMENT_BLOCK_CHARS {
        violations.push(LintViolation {
            code: LintCode::LongCommentBlock,
            severity: Severity::Error,
            subject: path.to_string(),
            message: format!(
                "`{path}` has a {chars}-character comment block, over the \
                 {LONG_COMMENT_BLOCK_CHARS}-character threshold — split it or trim it"
            ),
            line: Some(start),
        });
    }
}

/// Exactly one doc-comment line at the top, or `lint` refuses it. Only the leading run is
/// judged — a grid's own inline `; lane` comments stay out of scope.
fn doc_comment_violations(source: &dyn Source, graph: &Graph) -> Vec<LintViolation> {
    let mut violations = Vec::new();
    for path in graph.paths() {
        let Ok(Some(text)) = source.get(path) else {
            continue;
        };
        let header: Vec<&str> = text
            .lines()
            .take_while(|line| line.trim_start().starts_with(';'))
            .collect();
        match header.len() {
            0 => violations.push(LintViolation {
                code: LintCode::MissingComment,
                severity: Severity::Error,
                subject: path.to_string(),
                message: format!(
                    "`{path}` has no `;`-comment — every composition node must carry one `; \
                     Models: ... | Neglects: ... | IO: ... -> ... | Tags: ...` line \
                     documenting it"
                ),
                line: None,
            }),
            1 => {
                if let Err(reason) = parse_doc_comment(header[0]) {
                    violations.push(LintViolation {
                        code: LintCode::MalformedComment,
                        severity: Severity::Error,
                        subject: path.to_string(),
                        message: format!(
                            "`{path}`'s comment does not match `Models: ... | Neglects: ... \
                             | IO: ... -> ... | Tags: ...` ({reason})"
                        ),
                        line: None,
                    });
                }
            }
            n => violations.push(LintViolation {
                code: LintCode::MultilineComment,
                severity: Severity::Error,
                subject: path.to_string(),
                message: format!(
                    "`{path}` has {n} `;`-comment lines — exactly one is required, written \
                     denser instead of split across lines"
                ),
                line: None,
            }),
        }
    }
    violations
}

/// FORMAT 15.1 writes the shape as `Tags: <tag>[, <tag>]` and refuses a comment that does
/// not parse into its four fields. How many tags and what a tag looks like is house style,
/// so a numeral or a fourth tag is advised here and never refuses a composition.
fn tag_advice(line: &str) -> Option<String> {
    let held = parse_doc_comment(line).ok()?.tags;
    let odd: Vec<&str> = held
        .iter()
        .map(String::as_str)
        .filter(|t| !is_plain_tag(t))
        .collect();
    let mut notes = Vec::new();
    if held.len() > MAX_TAGS {
        notes.push(format!(
            "{} tags; {MAX_TAGS} keeps `Tags:` a triage aid rather than a second `Neglects:`",
            held.len()
        ));
    }
    if !odd.is_empty() {
        notes.push(format!(
            "`{}` reads as free text; a lowercase word of up to {MAX_TAG_CHARS} characters, \
             hyphens between segments, sorts and greps with the rest",
            odd.join("`, `")
        ));
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// One advisory per node whose tags are not house style.
fn tag_findings(source: &dyn Source, graph: &Graph) -> Vec<Finding> {
    let mut findings = Vec::new();
    for path in graph.paths() {
        let Ok(Some(text)) = source.get(path) else {
            continue;
        };
        let Some(line) = text.lines().find(|l| l.trim_start().starts_with(';')) else {
            continue;
        };
        if let Some(message) = tag_advice(line) {
            findings.push(Finding {
                code: LintCode::TagShape,
                severity: Severity::Advice,
                subject: path.to_string(),
                message: format!("`{path}`'s `Tags:` field: {message}"),
                line: None,
            });
        }
    }
    findings
}

/// Generous on purpose: today's longest real body is ~5000 chars — a backstop against
/// runaway generation, not a tight budget on complex physical models.
const EXPRESSION_BODY_CHARS: usize = 10_000;

/// A verbose local `@ref` name is a naming choice, not equation complexity.
const REF_PLACEHOLDER: &str = "@x";

/// Excludes the leading `;`-comment run. A node over budget on the raw count gets one more,
/// cheap recount with ref names collapsed, so a verbose `@ref` name alone cannot trip the cap.
fn long_expression_body_violations(source: &dyn Source, graph: &Graph) -> Vec<LintViolation> {
    let mut violations = Vec::new();
    for path in graph.paths() {
        let Ok(Some(text)) = source.get(path) else {
            continue;
        };
        let body: String = text
            .lines()
            .skip_while(|line| line.trim_start().starts_with(';'))
            .collect::<Vec<_>>()
            .join("\n");
        let chars = body.chars().count();
        if chars <= EXPRESSION_BODY_CHARS {
            continue;
        }
        let (chars, has_refs) = ref_stripped_char_count(&body);
        if chars > EXPRESSION_BODY_CHARS {
            let message = if has_refs {
                format!(
                    "`{path}` has a {chars}-character expression body even with every `@ref` \
                     name collapsed to `{REF_PLACEHOLDER}`, over the \
                     {EXPRESSION_BODY_CHARS}-character threshold — decompose it into sub-nodes"
                )
            } else {
                format!(
                    "`{path}` has a {chars}-character expression body, over the \
                     {EXPRESSION_BODY_CHARS}-character threshold — decompose it into sub-nodes"
                )
            };
            violations.push(LintViolation {
                code: LintCode::LongExpressionBody,
                severity: Severity::Error,
                subject: path.to_string(),
                message,
                line: None,
            });
        }
    }
    violations
}

/// Only reached once the raw count already exceeds the cap. Collapses each `@ref`'s `@path`
/// head to a placeholder; a trailing `(...)` invocation stays, being real expression content.
/// The `bool` says whether any ref was actually collapsed, so the error message can be worded
/// accurately for a body already over budget on pure literal content.
fn ref_stripped_char_count(body: &str) -> (usize, bool) {
    let spans = ref_spans(body);
    let has_refs = !spans.is_empty();
    let mut reduced = String::with_capacity(body.len());
    let mut cursor = 0;
    for span in spans {
        reduced.push_str(&body[cursor..span.start]);
        reduced.push_str(REF_PLACEHOLDER);
        cursor = span.end;
    }
    reduced.push_str(&body[cursor..]);
    (reduced.chars().count(), has_refs)
}

pub fn referenced(graph: &Graph) -> BTreeSet<String> {
    let mut reached: BTreeSet<String> = BTreeSet::new();
    for path in graph.paths() {
        if let Some(expr) = graph.expr(path) {
            collect_refs(path, expr, &mut reached);
        }
    }
    reached
}

/// Every node nothing references, `master` included — the roots a whole composition has.
pub fn entry_points(graph: &Graph) -> Vec<String> {
    let reached = referenced(graph);
    graph
        .paths()
        .filter(|p| !reached.contains(*p))
        .filter(|p| !sva_core::RESERVED_VARIABLES.contains(&p.rsplit('/').next().unwrap_or(p)))
        .map(str::to_string)
        .collect()
}

/// Not a waste warning — an unreached node costs no evaluation. It is what a typo looks like.
fn unreached(graph: &Graph) -> Vec<Finding> {
    entry_points(graph)
        .into_iter()
        .filter(|p| p != ROOT)
        .map(|p| Finding {
            code: LintCode::EntryPoint,
            severity: Severity::Advice,
            subject: p.to_string(),
            message: format!("nothing refs `{p}`, so it is an entry point — or a typo"),
            line: None,
        })
        .collect()
}

fn collect_refs(from: &str, expr: &Expr, out: &mut BTreeSet<String>) {
    if let Expr::Ref { path, .. } = expr
        && let Some(resolved) = resolve_ref_path(from, path)
    {
        out.insert(resolved);
    }
    for child in children(expr, Binds::Substitute) {
        collect_refs(from, child, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dir_of(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sva-cli-lint-{name}-{:x}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (rel, content) in files {
            fs::write(dir.join(rel), content).unwrap();
        }
        dir
    }

    /// A well-formed doc comment, for a fixture node the test isn't about.
    fn doc(models: &str) -> String {
        format!(
            "; Models: {models} | Neglects: nothing, it's a fixture | IO: t -> mix | Tags: \
             fixture\n"
        )
    }

    /// Both response paths are one external contract: a reader branches on `severity`, and
    /// gets the same object shape whether `lint` succeeded or refused.
    #[test]
    fn both_lint_response_paths_answer_the_same_diagnostic_shape() {
        let dir = dir_of(
            "one-envelope",
            &[
                ("master", &(doc("the mix") + "@kick*0.5\n")),
                ("kick", &(doc("a thump") + "sin(2*pi*50*t)\n")),
                ("spare", &(doc("a spare") + "sin(2*pi*80*t)\n")),
            ],
        );
        let report = lint(&dir, None).expect("a documented composition lints clean");
        let found: Vec<Diagnostic> = report.findings.iter().map(Finding::diagnostic).collect();
        let json = sva_core::success_envelope(
            &crate::output::lint_data(&dir.display().to_string(), None, report.nodes),
            &found,
        );
        assert!(json.contains("\"diagnostics\""), "{json}");
        assert!(json.contains("\"code\": \"lint.entry_point\""), "{json}");
        assert!(json.contains("\"severity\": \"advice\""), "{json}");
        assert!(
            json.contains("\"location\": { \"file\": \"spare\", \"span\": null, \"start\": null, \"end\": null }"),
            "{json}"
        );
        assert!(json.contains("\"help\""), "{json}");

        let undocumented = dir_of(
            "one-envelope-refused",
            &[("master", "@kick*0.5\n"), ("kick", "sin(t)\n")],
        );
        let Err(err) = lint(&undocumented, None) else {
            panic!("an undocumented node must refuse");
        };
        let refused = sva_core::error_envelope(err.code(), &err.message(), &err.diagnostics());
        assert!(refused.contains("\"diagnostics\""), "{refused}");
        assert!(
            refused.contains("\"code\": \"lint.missing_comment\""),
            "{refused}"
        );
        assert!(refused.contains("\"severity\": \"error\""), "{refused}");
        assert!(
            refused
                .contains("\"location\": { \"file\": \"kick\", \"span\": null, \"start\": null, \"end\": null }"),
            "{refused}"
        );
        assert!(refused.contains("\"help\""), "{refused}");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&undocumented);
    }

    /// The codes a clean-but-advised node reports, so a rule that only advises is asserted
    /// by what it says rather than by what it refuses.
    fn advised(result: Result<LintReport, CliError>, subject: &str) -> Vec<LintCode> {
        let report = result.unwrap_or_else(|e| panic!("`{subject}` should lint: {}", e.message()));
        report
            .findings
            .iter()
            .filter(|f| f.subject == subject)
            .map(|f| f.code)
            .collect()
    }

    fn assert_refused(result: Result<LintReport, CliError>, code: LintCode, subject: &str) {
        match result {
            Ok(report) => panic!(
                "expected a `{code:?}` refusal for `{subject}`, got a clean report: {:?}",
                report.findings.iter().map(|f| &f.code).collect::<Vec<_>>()
            ),
            Err(CliError::LintRefused(violations)) => assert!(
                violations
                    .iter()
                    .any(|v| v.code == code && v.subject == subject),
                "expected a `{code:?}` refusal for `{subject}`, got: {:?}",
                violations
                    .iter()
                    .map(|v| (v.code, &v.subject))
                    .collect::<Vec<_>>()
            ),
            Err(other) => panic!("expected a `{code:?}` refusal, got a different error: {other:?}"),
        }
    }

    #[test]
    fn a_loop_the_engine_can_schedule_lints_clean() {
        let dir = dir_of(
            "long-loop",
            &[
                (
                    "a",
                    &(doc("a fixture signal") + "sin(t) + @b(t - 0.01s)*0.5\n"),
                ),
                ("b", &(doc("a fixture signal") + "@a(t - 0.01s)*0.5\n")),
                ("master", &(doc("a fixture signal") + "@a\n")),
            ],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "a schedulable loop is not a finding"
        );
    }

    /// A target lints only what it reaches, so an orphan elsewhere in the directory is never
    /// seen — `unreached` does not run in this mode at all (see `lint`'s doc comment).
    #[test]
    fn a_target_skips_the_entry_point_check_a_whole_directory_lint_would_raise() {
        let dir = dir_of(
            "orphan",
            &[
                ("master", &(doc("a fixture signal") + "@drums\n")),
                ("drums", &(doc("a fixture signal") + "sin(t)\n")),
                ("orphan", &(doc("a fixture signal") + "sin(t)*0.5\n")),
            ],
        );
        let whole = lint(&dir, None).unwrap();
        assert!(
            whole
                .findings
                .iter()
                .any(|f| f.code == LintCode::EntryPoint && f.subject == "orphan"),
            "a whole-directory lint must flag the unreferenced `orphan`"
        );

        let targeted = lint(&dir, Some("master")).unwrap();
        assert!(
            !targeted
                .findings
                .iter()
                .any(|f| f.code == LintCode::EntryPoint),
            "a targeted lint must not run the entry-point check at all"
        );
    }

    /// The dogfooding bug: a trailing blank row makes 33, and 33/4 is not a whole subdivision —
    /// yet every intended note is still there, which is why counting notes alone missed it.
    #[test]
    fn a_grid_with_a_trailing_blank_row_over_its_bar_span_is_flagged() {
        let dir = dir_of(
            "trailing-blank-row",
            &[
                ("kick", &(doc("a fixture kick") + "sin(2*pi*50*t)\n")),
                (
                    "pattern-4b",
                    &(doc("a fixture pattern") + &("@kick\n".repeat(32) + "\n")),
                ),
                ("master", &(doc("a fixture signal") + "@pattern-4b\n")),
            ],
        );
        let whole = lint(&dir, None).unwrap();
        assert!(
            whole
                .findings
                .iter()
                .any(|f| f.code == LintCode::GridRowsPerBar && f.subject == "pattern-4b"),
            "33 rows over 4 bars must be flagged: {:?}",
            whole.findings.iter().map(|f| &f.code).collect::<Vec<_>>()
        );

        let targeted = lint(&dir, Some("master")).unwrap();
        assert!(
            targeted
                .findings
                .iter()
                .any(|f| f.code == LintCode::GridRowsPerBar),
            "the target-scoped path must run this check too"
        );
    }

    #[test]
    fn a_grid_whose_rows_tile_its_bar_span_evenly_is_clean() {
        let dir = dir_of(
            "clean-grid",
            &[
                ("kick", &(doc("a fixture kick") + "sin(2*pi*50*t)\n")),
                (
                    "pattern-4b",
                    &(doc("a fixture pattern") + &"@kick\n".repeat(32)),
                ),
                ("master", &(doc("a fixture signal") + "@pattern-4b\n")),
            ],
        );
        assert!(
            !lint(&dir, None)
                .unwrap()
                .findings
                .iter()
                .any(|f| f.code == LintCode::GridRowsPerBar),
            "32 rows over 4 bars is an exact subdivision"
        );
        assert!(
            !lint(&dir, Some("master"))
                .unwrap()
                .findings
                .iter()
                .any(|f| f.code == LintCode::GridRowsPerBar)
        );
    }

    #[test]
    fn a_grid_spanned_in_seconds_has_no_bar_count_to_check() {
        let dir = dir_of(
            "seconds-spanned",
            &[
                ("kick", &(doc("a fixture kick") + "sin(2*pi*50*t)\n")),
                (
                    "pattern-2s",
                    &(doc("a fixture pattern") + &("@kick\n".repeat(33) + "\n")),
                ),
                ("master", &(doc("a fixture signal") + "@pattern-2s\n")),
            ],
        );
        assert!(
            !lint(&dir, None)
                .unwrap()
                .findings
                .iter()
                .any(|f| f.code == LintCode::GridRowsPerBar),
            "a `-Ns` grid declares no bars, so there is nothing to divide"
        );
    }

    type ThresholdCase = (&'static str, std::path::PathBuf, Box<dyn Fn()>);

    #[test]
    fn every_length_threshold_sits_at_its_boundary_and_trips_one_char_over() {
        let cases: Vec<ThresholdCase> = vec![
            (
                "long-comment-block, 1000 chars",
                dir_of(
                    "boundary-comment",
                    &[(
                        "master",
                        &(doc("a fixture signal")
                            + "sin(t)\n"
                            + &format!(";{}", "x".repeat(999))
                            + "\n"),
                    )],
                ),
                Box::new(|| {
                    let trailing = format!(";{}", "x".repeat(1000));
                    let dir = dir_of(
                        "long-comment",
                        &[
                            (
                                "long",
                                &(doc("a fixture signal") + "sin(t)\n" + &trailing + "\n"),
                            ),
                            ("master", &(doc("a fixture signal") + "@long\n")),
                        ],
                    );
                    assert_refused(lint(&dir, None), LintCode::LongCommentBlock, "long");
                    assert_refused(
                        lint(&dir, Some("master")),
                        LintCode::LongCommentBlock,
                        "long",
                    );
                }) as Box<dyn Fn()>,
            ),
            (
                "long-expression-body, 10000 chars",
                dir_of(
                    "boundary-expression-body",
                    &[(
                        "drone",
                        &(String::from(
                            "; Models: a sustained drone | Neglects: envelope, detune | IO: t -> \
                             amplitude | Tags: drone\n",
                        ) + &"0".repeat(10_000)
                            + "\n"),
                    )],
                ),
                Box::new(|| {
                    let body = "0".repeat(10_001);
                    let dir = dir_of(
                        "long-expression-body",
                        &[
                            (
                                "drone",
                                &(String::from(
                                    "; Models: a sustained drone | Neglects: envelope, detune | \
                                     IO: t -> amplitude | Tags: drone\n",
                                ) + &body
                                    + "\n"),
                            ),
                            (
                                "master",
                                &(String::from(WELL_FORMED_MASTER_COMMENT) + "@drone\n"),
                            ),
                        ],
                    );
                    assert_refused(lint(&dir, None), LintCode::LongExpressionBody, "drone");
                    assert_refused(
                        lint(&dir, Some("master")),
                        LintCode::LongExpressionBody,
                        "drone",
                    );
                }),
            ),
            (
                "tag-shape, 24 chars",
                dir_of(
                    "tag-at-threshold",
                    &[("master", &(doc_with_tags(&"a".repeat(24)) + "sin(t)\n"))],
                ),
                Box::new(|| {
                    let dir = dir_of(
                        "tag-too-long",
                        &[("master", &(doc_with_tags(&"a".repeat(25)) + "sin(t)\n"))],
                    );
                    assert_eq!(
                        advised(lint(&dir, None), "master"),
                        vec![LintCode::TagShape]
                    );
                }),
            ),
        ];
        for (label, at, over) in cases {
            assert!(
                lint(&at, None).is_ok(),
                "{label}: exactly at the threshold, not over it"
            );
            over();
        }
    }

    /// Line 1's own run is exempt from this budget, however long its one required line is.
    #[test]
    fn a_long_but_well_formed_leading_doc_comment_is_not_a_long_comment_block() {
        let filler = "x".repeat(2000);
        let header = format!(
            "; Models: {filler} | Neglects: nothing, it's a fixture | IO: t -> mix | Tags: \
             fixture\n"
        );
        let dir = dir_of("long-header-exempt", &[("master", &(header + "sin(t)\n"))]);
        assert!(
            lint(&dir, None).is_ok(),
            "a single well-formed doc-comment line is exempt from the long-comment-block \
             budget regardless of its own length"
        );
    }

    /// A 2-line run is also `multiline-comment`; `.any()` isolates the arithmetic under test.
    #[test]
    fn the_line_one_header_annotation_is_excluded_from_its_run() {
        let header = ";header"; // 7 chars
        let boundary_line = format!(";{}", "x".repeat(999)); // 1000 chars
        let over_line = format!(";{}", "x".repeat(1000)); // 1001 chars

        let under = dir_of(
            "header-excluded-under",
            &[("master", &format!("{header}\n{boundary_line}\nsin(t)\n"))],
        );
        let Err(CliError::LintRefused(violations)) = lint(&under, None) else {
            panic!("a 2-line leading run is also multiline-comment, so this must still refuse")
        };
        assert!(
            !violations
                .iter()
                .any(|v| v.code == LintCode::LongCommentBlock),
            "1007 chars from line 1 is 1000 once the header is excluded: {:?}",
            violations.iter().map(|v| &v.code).collect::<Vec<_>>()
        );

        let over = dir_of(
            "header-excluded-over",
            &[("master", &format!("{header}\n{over_line}\nsin(t)\n"))],
        );
        assert_refused(lint(&over, None), LintCode::LongCommentBlock, "master");
    }

    /// Well-formed, so it never adds noise to a scenario testing some other node's comment.
    const WELL_FORMED_MASTER_COMMENT: &str = "; Models: a test harness's master node | Neglects: nothing, it's a fixture | IO: t -> mix | Tags: fixture\n";

    #[test]
    fn a_node_with_no_comment_at_all_is_missing_comment_in_both_modes() {
        let dir = dir_of(
            "missing-comment",
            &[
                ("plucked", "sin(t)\n"),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@plucked\n"),
                ),
            ],
        );
        assert_refused(lint(&dir, None), LintCode::MissingComment, "plucked");
        assert_refused(
            lint(&dir, Some("master")),
            LintCode::MissingComment,
            "plucked",
        );
    }

    #[test]
    fn two_contiguous_comment_lines_are_multiline_comment_in_both_modes() {
        let dir = dir_of(
            "multiline-comment",
            &[
                (
                    "stacked",
                    "; Models: a thing\n; Neglects: nothing | IO: t -> out\nsin(t)\n",
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@stacked\n"),
                ),
            ],
        );
        assert_refused(lint(&dir, None), LintCode::MultilineComment, "stacked");
        assert_refused(
            lint(&dir, Some("master")),
            LintCode::MultilineComment,
            "stacked",
        );
    }

    #[test]
    fn one_correctly_shaped_comment_line_under_the_length_threshold_is_clean() {
        let dir = dir_of(
            "well-formed-comment",
            &[
                (
                    "plucked",
                    "; Models: a plucked string's fundamental decay | Neglects: pick-position \
                     comb, body coupling | IO: note -> supersaw base | Tags: pluck\nsin(t)\n",
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@plucked\n"),
                ),
            ],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "a single well-formed, short comment must not refuse"
        );
        assert!(
            lint(&dir, Some("master")).is_ok(),
            "a single well-formed, short comment must not refuse"
        );
    }

    #[test]
    fn a_single_free_text_comment_line_is_malformed_comment_in_both_modes() {
        let dir = dir_of(
            "malformed-comment",
            &[
                (
                    "plucked",
                    "; a plucked string, decays over time, no body resonance modeled\nsin(t)\n",
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@plucked\n"),
                ),
            ],
        );
        assert_refused(lint(&dir, None), LintCode::MalformedComment, "plucked");
        assert_refused(
            lint(&dir, Some("master")),
            LintCode::MalformedComment,
            "plucked",
        );
    }

    /// `; lane`-style inline comments inside a TSV grid, well below the header, are a
    /// separate, already-legitimate feature (`sva-ast`'s `code_rows`) — not a second doc
    /// comment competing with the header for "exactly one".
    #[test]
    fn a_grids_own_inline_comment_below_a_well_formed_header_is_not_multiline_comment() {
        let dir = dir_of(
            "grid-inline-comment",
            &[
                ("kick", &(doc("a fixture kick") + "sin(2*pi*50*t)\n")),
                (
                    "pattern-1b",
                    "; Models: a kick pattern | Neglects: dynamics, humanization | \
                     IO: (t) -> amplitude | Tags: kick\n@kick\n; lane\n@kick\n",
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@pattern-1b\n"),
                ),
            ],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "a grid's own inline comment must not be mistaken for a second doc comment"
        );
    }

    /// A well-formed header plus a body just under the cap must not refuse, even though
    /// their combined length exceeds it.
    #[test]
    fn the_leading_doc_comment_is_excluded_from_the_expression_body_count() {
        let body = "0".repeat(9_999);
        let dir = dir_of(
            "doc-comment-excluded",
            &[(
                "drone",
                &(String::from(
                    "; Models: a sustained drone with a longer than usual doc comment header \
                     | Neglects: envelope, detune | IO: t -> amplitude | Tags: drone\n",
                ) + &body
                    + "\n"),
            )],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "the doc comment header must not count toward the body length"
        );
    }

    /// Raw count is over budget only because the author picked a verbose local `@ref` name;
    /// once every occurrence collapses to `@x` the equation itself is nowhere near the cap.
    #[test]
    fn a_body_over_budget_only_from_verbose_ref_names_is_clean_once_they_collapse() {
        let long_name = "a".repeat(50);
        let n = 200; // (1 + 50) chars per `@<name>` occurrence, joined by " + " (3 chars).
        let refs: Vec<String> = std::iter::repeat_n(format!("@{long_name}"), n).collect();
        let body = refs.join(" + ");
        let raw = body.chars().count();
        assert!(
            raw > EXPRESSION_BODY_CHARS,
            "raw count must be over budget: {raw}"
        );

        let reduced: usize = n * REF_PLACEHOLDER.chars().count() + (n - 1) * 3;
        assert!(
            reduced <= EXPRESSION_BODY_CHARS,
            "reduced count must be under budget: {reduced}"
        );

        let dir = dir_of(
            "ref-collapse-clean",
            &[
                (&long_name, &(doc("a fixture ref target") + "sin(t)\n")),
                (
                    "drone",
                    &(String::from(
                        "; Models: a sustained drone | Neglects: envelope, detune | IO: t -> \
                         amplitude | Tags: drone\n",
                    ) + &body
                        + "\n"),
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@drone\n"),
                ),
            ],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "a body over budget only from a verbose ref name must clear once ref names collapse"
        );
    }

    /// Even with ref names collapsed, enough literal filler keeps the reduced count over budget.
    #[test]
    fn a_body_still_over_budget_after_ref_collapse_is_still_flagged() {
        let long_name = "b".repeat(50);
        let filler = "0".repeat(10_500);
        let body = format!("@{long_name} + @{long_name} + {filler}");
        let raw = body.chars().count();
        assert!(
            raw > EXPRESSION_BODY_CHARS,
            "raw count must be over budget: {raw}"
        );
        let reduced = 2 * REF_PLACEHOLDER.chars().count() + 2 * 3 + filler.chars().count();
        assert!(
            reduced > EXPRESSION_BODY_CHARS,
            "reduced count must still be over budget: {reduced}"
        );

        let dir = dir_of(
            "ref-collapse-still-over",
            &[
                (&long_name, &(doc("a fixture ref target") + "sin(t)\n")),
                (
                    "drone",
                    &(String::from(
                        "; Models: a sustained drone | Neglects: envelope, detune | IO: t -> \
                         amplitude | Tags: drone\n",
                    ) + &body
                        + "\n"),
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@drone\n"),
                ),
            ],
        );
        assert_refused(lint(&dir, None), LintCode::LongExpressionBody, "drone");
    }

    /// A body with `@ref`s well under the raw cap is clean — refs or not, staying under
    /// budget never depends on collapsing them.
    #[test]
    fn a_body_with_refs_under_the_raw_cap_is_clean() {
        let dir = dir_of(
            "under-budget-with-refs",
            &[
                ("kick", &(doc("a fixture kick") + "sin(t)\n")),
                (
                    "drone",
                    &(String::from(
                        "; Models: a sustained drone | Neglects: envelope, detune | IO: t -> \
                         amplitude | Tags: drone\n",
                    ) + &"@kick + ".repeat(20)
                        + "0.5\n"),
                ),
                (
                    "master",
                    &(String::from(WELL_FORMED_MASTER_COMMENT) + "@drone\n"),
                ),
            ],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "this body is well under the raw cap"
        );
    }

    /// A well-formed doc comment with a caller-chosen `Tags:` value, for tests exercising
    /// `Tags:` validation specifically.
    fn doc_with_tags(tags: &str) -> String {
        format!(
            "; Models: a fixture signal | Neglects: nothing, it's a fixture | IO: t -> mix | \
             Tags: {tags}\n"
        )
    }

    #[test]
    fn a_comment_with_the_old_three_fields_and_no_tags_is_refused_as_missing_a_fourth_field() {
        let dir = dir_of(
            "old-three-field-comment",
            &[(
                "master",
                "; Models: a fixture signal | Neglects: nothing, it's a fixture | IO: t -> \
                 mix\nsin(t)\n",
            )],
        );
        let Err(CliError::LintRefused(violations)) = lint(&dir, None) else {
            panic!("a comment with no `Tags:` field must refuse")
        };
        let violation = violations
            .iter()
            .find(|v| v.code == LintCode::MalformedComment && v.subject == "master")
            .unwrap_or_else(|| {
                panic!(
                    "expected a malformed-comment refusal, got: {:?}",
                    violations.iter().map(|v| v.code).collect::<Vec<_>>()
                )
            });
        assert!(
            violation.message.contains("four"),
            "expected the refusal to name the missing fourth field: {}",
            violation.message
        );
    }

    #[test]
    fn a_tags_field_with_no_tags_is_refused() {
        let dir = dir_of(
            "empty-tags-field",
            &[("master", &(doc_with_tags("") + "sin(t)\n"))],
        );
        assert_refused(lint(&dir, None), LintCode::MalformedComment, "master");
    }

    #[test]
    fn a_tags_field_with_four_tags_is_advised() {
        let dir = dir_of(
            "four-tags",
            &[(
                "master",
                &(doc_with_tags("piano, sustained-pad, mellow, extra") + "sin(t)\n"),
            )],
        );
        assert_eq!(
            advised(lint(&dir, None), "master"),
            vec![LintCode::TagShape]
        );
    }

    #[test]
    fn a_tag_containing_a_space_is_advised() {
        let dir = dir_of(
            "tag-with-space",
            &[("master", &(doc_with_tags("sustained pad") + "sin(t)\n"))],
        );
        assert_eq!(
            advised(lint(&dir, None), "master"),
            vec![LintCode::TagShape]
        );
    }

    #[test]
    fn an_empty_tag_between_commas_is_refused() {
        let dir = dir_of(
            "empty-tag-between-commas",
            &[("master", &(doc_with_tags("piano,,pad") + "sin(t)\n"))],
        );
        assert_refused(lint(&dir, None), LintCode::MalformedComment, "master");
    }

    #[test]
    fn a_single_valid_tag_is_clean() {
        let dir = dir_of(
            "one-valid-tag",
            &[("master", &(doc_with_tags("piano") + "sin(t)\n"))],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "a single valid tag is not a finding"
        );
    }

    #[test]
    fn three_valid_comma_separated_tags_are_clean() {
        let dir = dir_of(
            "three-valid-tags",
            &[(
                "master",
                &(doc_with_tags("piano, sustained-pad, mellow") + "sin(t)\n"),
            )],
        );
        assert!(
            lint(&dir, None).is_ok(),
            "three valid comma-separated tags are not a finding"
        );
    }

    /// A well-formed 4-field header (valid `Tags:` included) ahead of a separate, non-leading
    /// comment block must not shield that block from `LONG_COMMENT_BLOCK_CHARS` — the two
    /// checks stay independent even once the header carries a fourth field.
    #[test]
    fn a_well_formed_tagged_header_does_not_exempt_a_later_long_comment_block() {
        let trailing = format!(";{}", "x".repeat(1000)); // 1001 chars
        let dir = dir_of(
            "tagged-header-long-block",
            &[
                (
                    "long",
                    &(doc_with_tags("fixture") + "sin(t)\n" + &trailing + "\n"),
                ),
                ("master", &(doc_with_tags("fixture") + "@long\n")),
            ],
        );
        assert_refused(lint(&dir, None), LintCode::LongCommentBlock, "long");
    }
}
