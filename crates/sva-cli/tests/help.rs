// Concern: states that `--help` prints every non-trivial default off the constant that decides it | Non-concern: what any flag does (args/), the JSON a subcommand answers | IO: () -> the help page

use sva_cli::help_text;

#[test]
fn help_states_the_sample_rate_default() {
    let page = help_text();
    assert!(
        page.contains(&format!("Default {}.", sva_engine::DEFAULT_SAMPLE_RATE)),
        "the rate a render actually uses: {page}"
    );
}

#[test]
fn help_states_the_depth_peaks_oversample_and_frame_defaults() {
    let page = help_text();
    let counts = [
        sva_core::DEFAULT_LEDGER_DEPTH,
        sva_core::DEFAULT_MAX_PEAKS,
        sva_core::DEFAULT_OVERSAMPLE as usize,
    ];
    for held in counts {
        assert!(page.contains(&format!("Default {held}")), "{held}: {page}");
    }
    assert!(
        page.contains(&format!("Default {}", sva_engine::DEFAULT_FRAME_SECS)),
        "the frame a pitch or stereo reading steps by: {page}"
    );
    for flag in [
        "--depth",
        "--peaks",
        "--oversample",
        "--frame",
        "--sample-rate",
    ] {
        assert!(
            page.contains(flag),
            "{flag} is named under DEFAULTS: {page}"
        );
    }
}

/// The two subcommands the standard's verb list has no word for.
#[test]
fn help_says_where_the_two_verbs_with_no_standard_name_sit() {
    let page = help_text();
    for named in ["render", "analyze"] {
        assert!(page.contains(named), "{named} is named under VERB ALIASES");
    }
    assert!(
        page.contains("no word for"),
        "and the page says why they keep their own names: {page}"
    );
}

/// The page is prose, and prose crosses the boundary as one escaped string, not as lines.
#[test]
fn help_answers_the_envelope_every_other_subcommand_answers() {
    let page = help_text();
    let envelope = sva_cli::success_envelope(&sva_cli::help_data(&page), &[]);
    assert!(
        envelope.starts_with("{\n  \"status\": \"success\","),
        "{envelope}"
    );
    assert!(
        envelope.contains(&format!("\"help\": \"{}\"", sva_core::json::escape(&page))),
        "the whole page, escaped: {envelope}"
    );
    assert!(
        page.lines().count() > envelope.lines().count(),
        "a multi-line page inside a one-line field"
    );
}
