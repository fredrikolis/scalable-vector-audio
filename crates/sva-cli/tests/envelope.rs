// Concern: proves every response carries one diagnostics collection, and both renderings of it agree | Non-concern: what a subcommand puts beside it | IO: (a data object) -> an envelope or lines

use sva_cli::{
    NAME, VERSION, builtins, builtins_data, diagnostics_text, help_data, help_text, lint_data,
    success_envelope,
};
use sva_core::{Diagnostic, Severity, error_envelope};

const EMPTY: &str = "\"diagnostics\": { \"items\": [], \"pagination\": { \"count\": 0, \
                     \"has_more\": false, \"next_cursor\": null } }";

/// A caller reads one field on every path. Absent on some and empty on others is two shapes,
/// and the standard's null convention admits one.
#[test]
fn every_response_carries_the_diagnostics_collection_found_or_not() {
    for data in [
        sva_cli::version_data(NAME, VERSION),
        help_data(&help_text()),
        builtins_data(&builtins()),
        lint_data("/tmp/song1", None, 3),
    ] {
        let envelope = success_envelope(&data, &[]);
        assert!(
            envelope.contains(EMPTY),
            "an empty collection, never a missing key: {}",
            &envelope[..envelope.len().min(400)]
        );
    }

    let found = success_envelope(
        &lint_data("/tmp/song1", None, 3),
        &[Diagnostic::new("lint.entry_point", "nothing refs `spare`")
            .with_severity(Severity::Advice)],
    );
    assert!(found.contains("\"code\": \"lint.entry_point\""), "{found}");
    assert!(found.contains("\"count\": 1"), "{found}");

    let refused = error_envelope(
        "validation_error",
        "a node without its comment",
        &[Diagnostic::new("lint.missing_comment", "no `;`-comment")],
    );
    assert!(refused.contains("\"diagnostics\""), "{refused}");
}

/// One set of objects, two renderings: a code the envelope carries is a code the terminal
/// view shows, and neither is reachable without asking for it.
#[test]
fn the_terminal_rendering_shows_the_diagnostics_the_envelope_carries() {
    let found = [
        Diagnostic::new("lint.entry_point", "nothing refs `spare`")
            .with_severity(Severity::Advice)
            .at(Some("spare".to_string()), None)
            .helped("ref it or delete it"),
        Diagnostic::new("lint.missing_comment", "no `;`-comment")
            .at(Some("kick".to_string()), None),
    ];

    let plain = diagnostics_text(&found, false);
    for held in &found {
        assert!(plain.contains(&held.code), "{plain}");
        assert!(plain.contains(&held.message), "{plain}");
    }
    assert!(
        plain.contains("advice["),
        "the severity it carries: {plain}"
    );
    assert!(plain.contains("spare"), "the node it located: {plain}");
    assert!(plain.contains("help: ref it or delete it"), "{plain}");
    assert!(
        plain.ends_with("2 diagnostic(s)"),
        "one count, no trailing line: {plain}"
    );
    assert!(
        !plain.contains('\u{1b}'),
        "no escape codes unasked: {plain}"
    );

    let colored = diagnostics_text(&found, true);
    assert!(colored.contains('\u{1b}'), "a terminal gets its colors");
    assert_eq!(
        colored.matches("lint.entry_point").count(),
        plain.matches("lint.entry_point").count(),
        "one rendering says no more than the other"
    );
}
