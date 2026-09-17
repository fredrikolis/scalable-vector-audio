// Concern: states what the `lint` code registry owes a caller branching on a code | Non-concern: which node earns which code (sva-cli lint.rs) | IO: (LintCode) -> asserted tag, help, diagnostic

use sva_core::{LintCode, Severity, lint_diagnostic};

#[test]
fn every_lint_code_answers_a_unique_tag_and_its_own_remediation() {
    let mut tags: Vec<&str> = LintCode::ALL.iter().map(|c| c.code_str()).collect();
    let before = tags.len();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(before, tags.len(), "a tag is one code's alone");

    for code in LintCode::ALL {
        let tag = code.code_str();
        assert!(
            tag.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "kebab-case only: {tag:?}"
        );
        assert!(!code.help().is_empty(), "{tag} says what to write instead");
    }
}

#[test]
fn a_lint_codes_diagnostic_is_namespaced_and_carries_that_remediation() {
    for code in LintCode::ALL {
        let d = lint_diagnostic(*code, "kick", "a message", Severity::Advice, Some(7));
        assert_eq!(
            d.code,
            format!("lint.{}", code.code_str()).replace('-', "_")
        );
        assert_eq!(d.help.as_deref(), Some(code.help()));
        assert_eq!(
            d.line,
            Some(7),
            "the line the check found it on carries through"
        );
    }
}
