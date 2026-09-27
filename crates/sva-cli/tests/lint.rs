// Concern: proves lint checks each file under its rules, and a target under its render's types | Non-concern: the per-node text checks (sva-cli lint.rs) | IO: (a composition) -> a report or a refusal

mod helpers;

use helpers::scratch;
use sva_cli::lint;
use sva_core::LintCode;

fn doc(models: &str) -> String {
    format!(
        "; Models: {models} | Neglects: nothing, it's a fixture | IO: t -> mix | Tags: fixture\n"
    )
}

fn composition(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = scratch(name);
    for (rel, body) in files {
        std::fs::write(dir.join(rel), format!("{}{body}", doc(rel))).expect("a node file");
    }
    dir
}

fn codes(report: &sva_cli::LintReport) -> String {
    report
        .findings
        .iter()
        .map(|f| format!("{} on {}", f.code.code_str(), f.subject))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A check that knows the line it found something on carries it into the diagnostic, so a
/// caller reading the response can open the file at it.
#[test]
fn a_lint_finding_names_its_line() {
    let dir = scratch("lint-line");
    let block = format!(";{}\n", "x".repeat(1_100));
    std::fs::write(
        dir.join("master"),
        format!(
            "{}sin(2*pi*220*t)\n{block}",
            doc("a tone under a long note")
        ),
    )
    .expect("a node file");

    let Err(refused) = lint(&dir, None) else {
        panic!("a comment block over the threshold refuses")
    };
    let found = refused.diagnostics();
    let [held] = &found[..] else {
        panic!("one block, one diagnostic, got {}", found.len())
    };
    assert_eq!(held.code, "lint.long_comment_block");
    assert_eq!(
        held.file.as_deref(),
        Some("master"),
        "the node it was found in"
    );
    assert_eq!(held.line, Some(3), "the line the run it found starts on");
}

/// A rate is what a render chooses, so a node written against one sounds different at every
/// other. The refusal names the `sp` spelling that holds at any of them.
#[test]
fn a_literal_sample_rate_refuses() {
    let dir = composition(
        "literal-rate",
        &[
            ("acc", "self(t - 1sp) + sample(sin(2*pi*220*t))/44100\n"),
            ("clean", "self(t - 1sp) + sample(sin(2*pi*220*t))*1sp\n"),
        ],
    );
    let Err(sva_core::CliError::LintRefused(found)) = lint(&dir, None) else {
        panic!("a literal rate refuses")
    };
    let refused: Vec<&str> = found
        .iter()
        .filter(|f| f.code == LintCode::LiteralSampleRate)
        .map(|f| f.subject.as_str())
        .collect();
    assert_eq!(refused, vec!["acc"], "only the node that wrote one");
    assert!(found[0].message.contains("1sp"), "the repair is named");
}

/// A reserved `variables/` node is read by name, so no ref walk reaches one.
#[test]
fn lint_of_one_node_sees_the_tempo() {
    let dir = composition(
        "one-node-tempo",
        &[("kick", "crop(sin(2*pi*50*t), 0s, 2b)\n")],
    );
    std::fs::create_dir_all(dir.join("variables")).expect("a variables directory");
    for (name, body) in [("bpm", "120\n"), ("meter", "4/4\n")] {
        std::fs::write(
            dir.join("variables").join(name),
            format!("{}{body}", doc(name)),
        )
        .expect("a variable");
    }

    let report = lint(&dir, Some("kick")).expect("one node lints under its own composition");
    assert_eq!(codes(&report), "", "nothing to report: {}", codes(&report));
}

/// A target names the instance its defaults resolved to, as `render` does.
#[test]
fn lint_of_a_node_with_a_default_resolves() {
    let dir = composition(
        "defaulted-node",
        &[
            ("plain", "sin(2*pi*440*t)\n"),
            ("voice", "f0 = 440\nsin(2*pi*f0*t)\n"),
        ],
    );

    assert_eq!(codes(&lint(&dir, Some("plain")).expect("a plain node")), "");
    let report = lint(&dir, Some("voice")).expect("a node that declares a default");
    assert_eq!(codes(&report), "", "{}", codes(&report));
    assert!(report.nodes > 0, "one node was checked, so one is reported");
}

/// A one-row body is no special case: the warning reads the same for any row count.
#[test]
fn a_one_row_grid_gets_the_same_warning_as_a_two_row_one() {
    let flagged = |name: &str, rows: usize| {
        let dir = composition(name, &[("kick", "sin(2*pi*50*t)\n")]);
        std::fs::write(
            dir.join("pattern-8b"),
            format!("{}{}", doc("a pattern"), "@kick\t@kick\n".repeat(rows)),
        )
        .expect("a grid");
        std::fs::write(
            dir.join("master"),
            format!("{}@pattern-8b\n", doc("a signal")),
        )
        .expect("a root");
        lint(&dir, None)
            .unwrap_or_else(|e| panic!("{name}: {}", e.message()))
            .findings
            .iter()
            .any(|f| f.code == LintCode::GridRowsPerBar)
    };

    assert!(!flagged("one-row-grid", 1), "one row is eight bars a row");
    assert!(!flagged("two-row-grid", 2), "two rows are four bars a row");
    assert!(!flagged("eight-row-grid", 8), "eight rows are one a bar");
    assert!(
        flagged("three-row-grid", 3),
        "three rows over eight bars tile it neither way"
    );
}

/// A typo'd target refuses under the code `render` answers a ref to it with.
#[test]
fn lint_of_an_undefined_target_refuses_like_render() {
    let dir = composition("undefined-target", &[("master", "sin(2*pi*300*t)\n")]);
    let Err(linted) = lint(&dir, Some("drums/kik")) else {
        panic!("a target nothing defines refuses");
    };
    let Err(rendered) = sva_core::probe(&dir, "@drums/kik") else {
        panic!("render refuses the ref that names it");
    };
    assert_eq!(linted.code(), rendered.code());
    assert_eq!(linted.exit_code(), rendered.exit_code());
}
