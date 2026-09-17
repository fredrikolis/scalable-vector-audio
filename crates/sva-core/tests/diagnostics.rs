// Concern: states how one engine refusal's reason, place and remediation reach a caller | Non-concern: which refusal a render raises (sva-engine's suites) | IO: (EngineError) -> a Diagnostic

use sva_core::CliError;

/// A diagnostic that runs the three together leaves a caller parsing prose for one of them.
#[test]
fn an_engine_refusal_answers_its_help_in_the_help_field_not_inside_the_message() {
    let refused = CliError::Engine(sva_engine::EngineError::refused(sva_engine::Diagnostic {
        code: "read.lines_need_unwindowed_lines".to_string(),
        message: "this term is windowed to [0s, 0.25s)".to_string(),
        location: sva_engine::Located::at("perc/hat", None),
        help: "`--as atoms` states each windowed term as it stands".to_string(),
    }));
    let [d] = &refused.diagnostics()[..] else {
        panic!("one refusal, one diagnostic");
    };
    assert_eq!(d.code, "read.lines_need_unwindowed_lines");
    assert_eq!(d.message, "this term is windowed to [0s, 0.25s)");
    assert_eq!(
        d.help.as_deref(),
        Some("`--as atoms` states each windowed term as it stands")
    );
    assert_eq!(d.file.as_deref(), Some("perc/hat"));

    let named = CliError::Engine(sva_engine::EngineError::UnknownNode("nope".to_string()));
    let [d] = &named.diagnostics()[..] else {
        panic!("one refusal, one diagnostic");
    };
    assert_eq!(d.code, "engine.unknown_node");
    assert_eq!(d.message, "no such node: `nope`");
    assert!(d.help.is_none(), "a variant with no remediation names none");
}
