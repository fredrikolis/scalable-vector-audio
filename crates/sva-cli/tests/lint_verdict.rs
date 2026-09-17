// Concern: proves a lint verdict answers every finding the scan made | Non-concern: which node earns which finding (the rest of the suite) | IO: (a composition) -> a verdict + diagnostics

mod helpers;

use helpers::scratch;
use sva_cli::lint;

fn doc(models: &str) -> String {
    format!(
        "; Models: {models} | Neglects: nothing, it's a fixture | IO: t -> mix | Tags: fixture\n"
    )
}

/// The refusal used to raise before every advisory pass, so one malformed comment hid the rest.
#[test]
fn a_refusing_lint_still_lists_its_advisories() {
    let dir = scratch("refusing-with-advice");
    std::fs::write(dir.join("master"), "@tone*0.5\n").expect("a node with no doc comment");
    std::fs::write(
        dir.join("tone"),
        format!("{}sin(2*pi*44100*t)\n", doc("tone")),
    )
    .expect("a node written against the rate");
    std::fs::write(
        dir.join("spare"),
        format!("{}sin(2*pi*110*t)\n", doc("spare")),
    )
    .expect("a node nothing references");

    let Err(refused) = lint(&dir, None) else {
        panic!("a node with no `;`-comment refuses");
    };
    let answered = refused.diagnostics();
    let found: Vec<(&str, sva_core::Severity)> = answered
        .iter()
        .map(|d| (d.code.as_str(), d.severity))
        .collect();
    assert!(
        found.contains(&("lint.missing_comment", sva_core::Severity::Error)),
        "{found:?}"
    );
    for advised in ["lint.entry_point", "lint.literal_sample_rate"] {
        assert!(
            found.contains(&(advised, sva_core::Severity::Advice)),
            "{advised} is answered beside the refusal: {found:?}"
        );
    }
    assert_eq!(
        refused.message(),
        "1 lint violation(s)",
        "the verdict counts what refused, not what was advised"
    );
}

/// A target's own reach refuses through a second path, which used to return before folding in
/// what the same scan had already found.
#[test]
fn a_refusing_targeted_lint_still_lists_its_advisories() {
    let dir = scratch("refusing-target");
    std::fs::write(dir.join("tone"), "sin(2*pi*44100*hz*t)\n").expect("a node with no comment");
    std::fs::write(
        dir.join("master"),
        format!("{}@tone(t, hz=1)*0.5 + @tone(t, hz=2)*0.5\n", doc("master")),
    )
    .expect("two call sites");

    let Err(refused) = lint(&dir, Some("tone")) else {
        panic!("a node with no `;`-comment refuses, whatever else its file is");
    };
    let answered = refused.diagnostics();
    let found: Vec<&str> = answered.iter().map(|d| d.code.as_str()).collect();
    assert!(found.contains(&"lint.missing_comment"), "{found:?}");
    assert!(found.contains(&"lint.literal_sample_rate"), "{found:?}");
}
