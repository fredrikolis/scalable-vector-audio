// Concern: proves a whole-directory lint decides the same types a targeted one does | Non-concern: the per-node text checks (sva-cli lint.rs) | IO: (a composition) -> a report or a refusal

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

fn refused(report: &sva_cli::LintReport, subject: &str) -> String {
    report
        .findings
        .iter()
        .find(|f| f.code == LintCode::EntryPointRefused && f.subject == subject)
        .unwrap_or_else(|| panic!("`{subject}` was typed: {}", codes(report)))
        .message
        .clone()
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

/// A crossing in a rootless graph is still a crossing.
#[test]
fn a_rootless_composition_still_gets_structural_findings() {
    let dir = composition(
        "rootless-structure",
        &[
            ("solver", "chaigne_askenfelt(261.63)\n"),
            ("mixed", "@solver*sin(2*pi*3*t)\n"),
        ],
    );
    let report = lint(&dir, None).expect("a whole-directory lint reports what it found");
    let text = refused(&report, "mixed");
    assert!(
        text.contains("samples"),
        "the finding names the crossing: {text}"
    );

    let clean = composition(
        "rootless-clean",
        &[("one", "sin(2*pi*300*t)\n"), ("two", "sin(2*pi*400*t)\n")],
    );
    let report = lint(&clean, None).expect("two typed entry points lint clean");
    assert_eq!(report.nodes, 2);
    assert!(
        !report
            .findings
            .iter()
            .any(|f| f.code == LintCode::EntryPointRefused),
        "nothing to report: {}",
        codes(&report)
    );
}

/// Not the first entry point, every one: `alpha` types, and the crossing is in `omega`.
#[test]
fn a_later_entry_point_is_typed_too() {
    let dir = composition(
        "later-entry-point",
        &[
            ("alpha", "sin(2*pi*300*t)\n"),
            ("solver", "chaigne_askenfelt(261.63)\n"),
            ("omega", "@solver*sin(2*pi*3*t)\n"),
        ],
    );
    let report = lint(&dir, None).expect("a report, not a refusal");
    assert!(refused(&report, "omega").contains("samples"));
}

/// A `master` is one root among the entry points, not the only one that gets typed.
#[test]
fn an_orphan_beside_a_master_is_typed_too() {
    let dir = composition(
        "orphan-beside-master",
        &[
            ("master", "sin(2*pi*300*t)\n"),
            ("solver", "chaigne_askenfelt(261.63)\n"),
            ("orphan", "@solver*sin(2*pi*3*t)\n"),
        ],
    );
    let report = lint(&dir, None).expect("a master that types is not a refusal");
    assert!(refused(&report, "orphan").contains("samples"));
}

/// A template nothing binds cannot be typed on its own, and saying so is not the same as
/// refusing the directory it sits in.
#[test]
fn an_unbound_template_is_reported_not_refused() {
    let dir = composition("unbound-template", &[("voice", "sin(2*pi*f0*t)*v\n")]);
    let report = lint(&dir, None).expect("one unbindable template is not a broken directory");
    assert!(refused(&report, "voice").contains("free"));
}

/// A rate is what a render chooses, so a node written against one sounds different at every
/// other. The advisory names the `sp` spelling that holds at any of them.
#[test]
fn a_literal_sample_rate_is_advised() {
    let dir = composition(
        "literal-rate",
        &[
            ("acc", "self(t - 1sp) + sample(sin(2*pi*220*t))/44100\n"),
            ("clean", "self(t - 1sp) + sample(sin(2*pi*220*t))*1sp\n"),
        ],
    );
    let report = lint(&dir, None).expect("a literal rate is advice, never a refusal");
    let advised: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.code == LintCode::LiteralSampleRate)
        .map(|f| f.subject.as_str())
        .collect();
    assert_eq!(advised, vec!["acc"], "only the node that wrote one");
    let message = &report
        .findings
        .iter()
        .find(|f| f.code == LintCode::LiteralSampleRate)
        .expect("the advisory")
        .message;
    assert!(message.contains("1sp"), "the repair is named: {message}");
}

/// FORMAT 15.1: a composition is a directory of node files. A file no ref can name is not
/// one of them, so it is passed over and said aloud rather than refused.
#[test]
fn a_file_beside_nodes_is_advised_not_refused() {
    let dir = composition("beside-nodes", &[("master", "sin(2*pi*220*t)\n")]);
    std::fs::write(dir.join("take 1.wav"), "not a node\n").expect("a rendering");
    std::fs::write(dir.join(".hidden"), "nor is this\n").expect("a dotfile");

    let report = lint(&dir, None).expect("a note beside a node is no refusal");
    assert_eq!(
        report.nodes, 1,
        "one node, whatever else the directory holds"
    );
    let advised: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.code == LintCode::NotANode)
        .map(|f| f.subject.as_str())
        .collect();
    assert!(advised.contains(&"take 1.wav"), "{advised:?}");
    assert!(advised.contains(&".hidden"), "{advised:?}");
    assert!(
        report
            .findings
            .iter()
            .all(|f| f.severity != sva_core::Severity::Error),
        "nothing here is an error"
    );
}

/// The advice exists to catch a filename that went wrong. A dot directory belongs to a tool
/// and a `.md` or `.json` sibling is a document a composer wrote: neither went wrong, and a
/// pass that says so once per file in `.git` says nothing at all.
#[test]
fn a_markdown_sibling_and_git_internals_get_no_lint_row() {
    let dir = composition("quiet-siblings", &[("master", "sin(2*pi*220*t)\n")]);
    std::fs::write(dir.join("LEARNED.md"), "# what I learned\n").expect("a note");
    std::fs::write(dir.join("TARGETS.json"), "{}\n").expect("a reference reading");
    std::fs::create_dir_all(dir.join(".git/hooks")).expect("a tool's own directory");
    std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/master\n").expect("tool state");
    std::fs::write(dir.join(".git/hooks/pre-commit.sample"), "#!/bin/sh\n").expect("a sample");

    let report = lint(&dir, None).expect("a document beside a node is no refusal");
    let advised: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.code == LintCode::NotANode)
        .map(|f| f.subject.as_str())
        .collect();
    assert!(advised.is_empty(), "nothing here went wrong: {advised:?}");
    assert_eq!(report.nodes, 1, "one node, whatever else sits beside it");
}

/// A composer names a note whatever they like, and `NOTES.MD` is the same document as
/// `notes.md`.
#[test]
fn an_uppercase_notes_md_gets_no_lint_row() {
    let dir = composition("shouted-siblings", &[("master", "sin(2*pi*220*t)\n")]);
    std::fs::write(dir.join("NOTES.MD"), "# what I learned\n").expect("a note");
    std::fs::write(dir.join("Targets.Json"), "{}\n").expect("a reference reading");

    let report = lint(&dir, None).expect("a document beside a node is no refusal");
    let advised: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.code == LintCode::NotANode)
        .map(|f| f.subject.as_str())
        .collect();
    assert!(advised.is_empty(), "a suffix is a suffix: {advised:?}");
    assert_eq!(report.nodes, 1, "one node, whatever else sits beside it");
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

/// FORMAT 15.1 states no tag count and no case, so neither refuses a composition.
#[test]
fn an_uppercase_numeral_tag_is_advised_not_refused() {
    let dir = scratch("numeral-tag");
    std::fs::write(
        dir.join("master"),
        "; Models: the V chord | Neglects: voicing | IO: t -> mix | Tags: harmony, \
         numeral-V, chord, spine\nsin(2*pi*440*t)\n",
    )
    .expect("a node file");

    let report = lint(&dir, None).expect("a numeral tag is not a broken composition");
    let advised: Vec<&sva_cli::Finding> = report
        .findings
        .iter()
        .filter(|f| f.code == LintCode::TagShape)
        .collect();
    let [found] = advised.as_slice() else {
        panic!("one advisory about the tags: {}", codes(&report));
    };
    assert!(
        matches!(found.severity, sva_core::Severity::Advice),
        "a tag's shape is house style, not a refusal"
    );
    assert!(
        found.message.contains("numeral-V"),
        "the advisory names the tag it is about: {}",
        found.message
    );
}

/// A one-row body is no special case: the advice reads the same for any row count.
#[test]
fn a_one_row_grid_gets_the_same_advice_as_a_two_row_one() {
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

/// A ramp reaches zero at its own edge, so a transient written under one is multiplied by
/// nearly nothing. Nothing refuses it and the node's own rms still looks plausible, so the
/// advisory names the window and the ramp that swallowed it.
#[test]
fn a_short_strike_under_a_long_shoulder_is_advised() {
    let dir = composition(
        "strike-under-shoulder",
        &[
            ("strike", "crop(sin(2*pi*440*t)*exp(-t/0.01), 0s, 0.05s)\n"),
            ("master", "crop(@strike(t), 0s, 4s, rise=2s, fall=1s)\n"),
        ],
    );
    let report = lint(&dir, None).expect("an advisory is no refusal");
    let found = report
        .findings
        .iter()
        .find(|f| f.code == LintCode::WindowInsideRamp)
        .unwrap_or_else(|| panic!("the ramp swallowed the strike: {}", codes(&report)));
    assert_eq!(found.subject, "master");
    assert_eq!(found.severity, sva_core::Severity::Advice);
    assert!(
        found.message.contains("0.0500 s window"),
        "{}",
        found.message
    );
    assert!(found.message.contains("2.0000 s ramp"), "{}", found.message);

    // A warp reads its operand at a time no shift states, so nothing under one is placed.
    let warped = composition(
        "strike-under-a-warp",
        &[
            ("strike", "crop(sin(2*pi*440*t)*exp(-t/0.01), 0s, 0.05s)\n"),
            ("master", "crop(@strike(t*2), 0s, 4s, rise=2s, fall=1s)\n"),
        ],
    );
    let unplaced = lint(&warped, None).expect("a warp is no refusal");
    assert!(
        !unplaced
            .findings
            .iter()
            .any(|f| f.code == LintCode::WindowInsideRamp),
        "a warped window is placed nowhere: {}",
        codes(&unplaced)
    );

    // A window past the ramp's own half-way turn is over half amplitude, which is shaping.
    let shaped = composition(
        "strike-past-the-turn",
        &[
            ("strike", "crop(sin(2*pi*440*t)*exp(-t/0.01), 0s, 1.5s)\n"),
            ("master", "crop(@strike(t), 0s, 4s, rise=2s, fall=1s)\n"),
        ],
    );
    let quiet = lint(&shaped, None).expect("a shaped window is no advisory");
    assert!(
        !quiet
            .findings
            .iter()
            .any(|f| f.code == LintCode::WindowInsideRamp),
        "{}",
        codes(&quiet)
    );
}

/// A typo'd target refuses under the code and message `render` answers the same input with.
#[test]
fn lint_of_an_undefined_target_refuses_like_render() {
    let dir = composition("undefined-target", &[("master", "sin(2*pi*300*t)\n")]);
    let Err(linted) = lint(&dir, Some("drums/kik")) else {
        panic!("a target nothing defines refuses");
    };
    let Err(rendered) = sva_core::probe(&dir, "drums/kik") else {
        panic!("render refuses the same input");
    };
    assert_eq!(linted.code(), rendered.code());
    assert_eq!(linted.message(), rendered.message());
    assert_eq!(linted.exit_code(), rendered.exit_code());
}

/// Opening one would block, so the walk never does; it used to say so on stderr alone, where
/// no JSON-reading caller could see it.
#[test]
#[cfg(unix)]
fn a_skipped_special_file_is_reported_as_a_diagnostic() {
    let dir = composition("special-file", &[("master", "sin(2*pi*220*t)\n")]);
    let fifo = dir.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo failed");

    let report = lint(&dir, None).expect("a FIFO beside a node is no refusal");
    assert_eq!(report.nodes, 1, "the node beside it still loads");
    let found = report
        .findings
        .iter()
        .find(|f| f.subject == "pipe")
        .unwrap_or_else(|| panic!("the FIFO is named: {}", codes(&report)));
    assert_eq!(found.code, LintCode::NotAFile);
    assert_eq!(found.severity, sva_core::Severity::Advice);
    let diagnostic = found.diagnostic();
    assert_eq!(diagnostic.code, "lint.not_a_file");
    assert_eq!(diagnostic.help.as_deref(), Some(LintCode::NotAFile.help()));
}

/// `lint` used to load only what its target reaches, so a file with parameters was read as a
/// node with a free one; `render` and `trace` both name the instances the composition holds.
#[test]
fn lint_of_a_parameterized_file_lists_its_instances() {
    let dir = composition(
        "parameterized",
        &[
            ("tone", "sin(2*pi*hz*t)\n"),
            ("master", "@tone(t, hz=220)*0.5 + @tone(t, hz=440)*0.5\n"),
        ],
    );

    let Err(refused) = lint(&dir, Some("tone")) else {
        panic!("a file with two instances is no node of its own");
    };
    let message = refused.message();
    for held in ["tone(hz=220)", "tone(hz=440)"] {
        assert!(message.contains(held), "{message}");
    }
    assert_eq!(
        refused.diagnostics()[0].code,
        "engine.ambiguous_node",
        "{message}"
    );

    let one = composition("parameterized-once", &[("tone", "sin(2*pi*hz*t)\n")]);
    std::fs::write(
        one.join("master"),
        format!("{}@tone(t, hz=220)\n", doc("master")),
    )
    .expect("a single call site");
    let report = lint(&one, Some("tone")).expect("one instance is the node the caller meant");
    assert_eq!(report.nodes, 2, "the instance and what calls it");
}
