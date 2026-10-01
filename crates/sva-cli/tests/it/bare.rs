// Concern: proves lint warns of a node a bare `@path` cannot play: a parameter with no default, a support with no end | Non-concern: the other lint checks | IO: (a composition) -> findings

use crate::helpers::{doc, scratch};
use sva_cli::lint;
use sva_core::{LintCode, Severity};

fn composition(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = scratch(name);
    for (rel, body) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().expect("inside the scratch dir")).expect("a dir");
        std::fs::write(path, format!("{}{body}", doc(rel))).expect("a node file");
    }
    dir
}

/// Each `(subject, message)` lint found under `code`, every one a warning.
fn found(dir: &std::path::Path, code: LintCode) -> Vec<(String, String)> {
    let report = lint(dir, None).unwrap_or_else(|e| panic!("lint refused: {}", e.message()));
    report
        .findings
        .iter()
        .filter(|f| f.code == code)
        .inspect(|f| assert_eq!(f.severity, Severity::Warning, "{}", f.message))
        .map(|f| (f.subject.clone(), f.message.clone()))
        .collect()
}

#[test]
fn a_parameter_with_no_default_line_is_named_with_its_file() {
    let dir = composition("no-default", &[("tone", "crop(sin(2*pi*f0*t), 0s, 1s)\n")]);
    let held = found(&dir, LintCode::ParameterHasNoDefault);
    let [(subject, message)] = held.as_slice() else {
        panic!("one parameter, one finding: {held:?}");
    };
    assert_eq!(subject, "tone");
    assert!(message.contains("`f0`"), "{message}");
    assert!(
        message.contains("f0 = "),
        "names the line to write: {message}"
    );

    let defaulted = composition(
        "with-default",
        &[("tone", "f0 = C4\ncrop(sin(2*pi*f0*t), 0s, 1s)\n")],
    );
    assert_eq!(found(&defaulted, LintCode::ParameterHasNoDefault), []);
}

/// What the language answers itself is no parameter: `t`, constants, note names, units,
/// builtins, a loop's `self`, a series index and the names a call's arguments are given.
#[test]
fn the_languages_own_names_are_not_parameters() {
    let dir = composition(
        "language-names",
        &[
            (
                "tone",
                "gain = -6db\ncrop(lowpass(sample(sin(2*pi*A4*t) + sum(k, 1, 3, sin(2*pi*k*C4*t)/k)), \
                 cutoff=800hz, q=0.7)*gain + 0.5*self[idx(t) - 1] + 0*pi, 0s, 1s)\n",
            ),
            ("open", "crop(sin(2*pi*C4*t), 0s, inf)\n"),
            ("voice", "f0 = 220hz\ncrop(sin(2*pi*f0*t), 0s, 50ms)\n"),
            ("chord", "@voice(t, f0=E4) + @voice(t - 10ms, f0=G4)\n"),
        ],
    );
    assert_eq!(found(&dir, LintCode::ParameterHasNoDefault), []);
}

#[test]
fn a_parameter_read_only_through_a_calls_named_argument_is_found() {
    let dir = composition(
        "named-only",
        &[
            ("src", "crop(sin(2*pi*440*t), 0s, 1s)\n"),
            ("filtered", "lowpass(sample(@src), cutoff=fc)\n"),
            ("voice", "f0 = 220hz\ncrop(sin(2*pi*f0*t), 0s, 50ms)\n"),
            ("line", "@voice(t, f0=pitch)\n"),
        ],
    );
    let held = found(&dir, LintCode::ParameterHasNoDefault);
    let named: Vec<(&str, bool)> = held
        .iter()
        .map(|(s, m)| (s.as_str(), m.contains("`fc`") || m.contains("`pitch`")))
        .collect();
    assert_eq!(named, [("filtered", true), ("line", true)], "{held:?}");
    assert!(
        held.iter()
            .all(|(_, m)| !m.contains("`cutoff`") && !m.contains("`f0`")),
        "an argument's own name is the callee's: {held:?}"
    );
}

#[test]
fn every_parameter_a_node_leaves_unbound_is_named() {
    let dir = composition(
        "two-free",
        &[("tone", "crop(amp*sin(2*pi*f0*t), 0s, 1s)\n")],
    );
    let mut names: Vec<String> = found(&dir, LintCode::ParameterHasNoDefault)
        .into_iter()
        .map(|(_, m)| m)
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(
        names[0].contains("`amp`") && names[1].contains("`f0`"),
        "{names:?}"
    );
}

#[test]
fn a_node_whose_support_never_ends_is_warned_of() {
    let dir = composition("endless", &[("drone", "sin(2*pi*110*t)\n")]);
    let held = found(&dir, LintCode::SupportNeverEnds);
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].0, "drone");

    let cropped = composition("cropped", &[("drone", "crop(sin(2*pi*110*t), 0s, 2s)\n")]);
    assert_eq!(found(&cropped, LintCode::SupportNeverEnds), []);
}

/// A decay never reaches zero, yet the prune the render already takes ends it.
#[test]
fn a_decay_the_prune_ends_is_bounded() {
    let dir = composition(
        "decays",
        &[("pluck", "step(t)*exp(-t/0.1s)*sin(2*pi*440*t)\n")],
    );
    assert_eq!(found(&dir, LintCode::SupportNeverEnds), []);
}

/// A node reading an endless one ends only where it says so.
#[test]
fn a_reader_inherits_an_endless_support_unless_it_crops_it() {
    let dir = composition(
        "inherited",
        &[
            ("drone", "sin(2*pi*110*t)\n"),
            ("loud", "@drone*0.5\n"),
            ("short", "crop(@drone, 0s, 1s)\n"),
        ],
    );
    let mut subjects: Vec<String> = found(&dir, LintCode::SupportNeverEnds)
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    subjects.sort();
    assert_eq!(subjects, ["drone", "loud"]);
}

/// A reserved variable is read by name, never played.
#[test]
fn a_reserved_variable_is_no_sound() {
    let dir = composition(
        "reserved",
        &[
            ("variables/key", "C4\n"),
            ("tone", "crop(sin(2*pi*@variables/key*t), 0s, 1s)\n"),
        ],
    );
    let report = lint(&dir, None).unwrap_or_else(|e| panic!("{}", e.message()));
    let codes: Vec<(&str, &str)> = report
        .findings
        .iter()
        .map(|f| (f.code.code_str(), f.subject.as_str()))
        .collect();
    assert_eq!(codes, [] as [(&str, &str); 0], "{codes:?}");
}
