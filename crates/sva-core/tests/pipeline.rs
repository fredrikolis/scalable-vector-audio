// Concern: states what the shared pipeline answers a front end handing it argv text | Non-concern: what a render sounds like (sva-engine's suites) | IO: (text) -> a window, roots, a config

use sva_core::{
    CliError, Job, WindowEdge, config_for, execute, prepared, roots_of, window_edge, window_for,
};

fn composition(files: &[(&str, &str)]) -> sva_ast::Composition {
    files.iter().copied().collect()
}

/// The only place either front end turns a `--from`/`--to` into a number.
#[test]
fn a_window_edge_reads_the_time_literals_and_refuses_anything_else() {
    assert_eq!(window_edge("0.5", "--from").unwrap(), WindowEdge::Secs(0.5));
    assert_eq!(window_edge("2s", "--to").unwrap(), WindowEdge::Secs(2.0));
    assert_eq!(window_edge("3b", "--to").unwrap(), WindowEdge::Bars(3.0));
    assert_eq!(window_edge("end", "--to").unwrap(), WindowEdge::End);
    assert_eq!(window_edge("0", "--from").unwrap(), WindowEdge::Secs(0.0));

    for refused in ["", "-1", "1x", "nan", "inf", "1..2", "s", "b"] {
        let held = window_edge(refused, "--from");
        assert!(
            matches!(held, Err(CliError::Usage(_))),
            "`{refused}` is not a time: {held:?}"
        );
    }
}

/// A bar is a duration only a tempo settles, so one without a tempo refuses.
#[test]
fn a_bar_edge_needs_a_tempo_and_seconds_never_do() {
    let held =
        window_for(None, Some(WindowEdge::Bars(2.0)), Some(1.5)).expect("a tempo settles it");
    assert_eq!(held.start_secs, 0.0);
    assert_eq!(held.end_secs, 3.0, "two bars of 1.5s");

    assert!(matches!(
        window_for(None, Some(WindowEdge::Bars(2.0)), None),
        Err(CliError::BadTempo(_))
    ));
    let plain = window_for(
        Some(WindowEdge::Secs(0.25)),
        Some(WindowEdge::Secs(0.75)),
        None,
    )
    .expect("seconds need no tempo");
    assert_eq!((plain.start_secs, plain.end_secs), (0.25, 0.75));
}

/// An unstated edge is the node's own extent, and a stated one is what it gets instead.
#[test]
fn a_config_takes_the_horizon_from_the_node_until_a_caller_names_one() {
    let source = composition(&[("master", "crop(sin(2*pi*440*t), 0s, 2s)\n")]);
    let graph = prepared(&source).expect("a composition that parses");

    let own = config_for(&graph, "master", None, None, None).expect("its own extent");
    assert_eq!(own.horizon.end_secs, 2.0);
    assert_eq!(own.rate, 44_100, "the default observation rate");

    let asked = config_for(
        &graph,
        "master",
        Some(WindowEdge::Secs(0.5)),
        Some(WindowEdge::Secs(1.0)),
        Some(96_000),
    )
    .expect("the window the caller named");
    assert_eq!(
        (asked.horizon.start_secs, asked.horizon.end_secs),
        (0.5, 1.0)
    );
    assert_eq!(asked.rate, 96_000);

    assert!(
        matches!(
            config_for(
                &graph,
                "master",
                Some(WindowEdge::Secs(1.0)),
                Some(WindowEdge::Secs(0.5)),
                None
            ),
            Err(CliError::Usage(_))
        ),
        "a window that ends before it starts is no window"
    );
}

/// A target the source answers for is a node; anything else is argv math.
#[test]
fn the_roots_of_a_target_are_the_node_or_what_its_math_reads() {
    let source = composition(&[("master", "@kick\n"), ("kick", "sin(2*pi*60*t)\n")]);
    let holds = |target, name: &str| {
        roots_of(&source, target)
            .unwrap()
            .contains(&name.to_string())
    };

    assert!(holds(None, "master"), "no target renders `master`");
    assert!(holds(Some("kick"), "kick"), "a node is its own root");
    assert!(
        holds(Some("@kick*0.5"), "kick"),
        "argv math pulls what it reads"
    );
    assert!(
        holds(Some("sin(t)"), "bpm"),
        "and every reserved variable, which no ref walk reaches"
    );
}

/// A node's extent is the widest window anything under it crops to, and a crop reaching
/// before zero is pre-roll the reading starts at.
#[test]
fn an_extent_is_the_widest_crop_under_the_node_not_the_roots_own() {
    let source = composition(&[
        ("early", "crop(sin(2*pi*220*t), 0s - 0.5s, 1s)\n"),
        ("late", "crop(sin(2*pi*440*t), 0.25s, 3s)\n"),
        ("master", "@early + @late\n"),
    ]);
    let graph = prepared(&source).expect("a composition that parses");
    let held = config_for(&graph, "master", None, None, None).expect("its own extent");
    assert_eq!(held.horizon.end_secs, 3.0, "the furthest end under it");
    assert_eq!(held.horizon.start_secs, -0.5, "the earliest start under it");

    let plain = composition(&[("master", "crop(sin(2*pi*440*t), 0.25s, 2s)\n")]);
    let graph = prepared(&plain).expect("a composition that parses");
    let held = config_for(&graph, "master", None, None, None).expect("its own extent");
    assert_eq!(
        held.horizon.start_secs, 0.0,
        "a crop after zero is no pre-roll"
    );
}

/// `bpm` and `meter` come as a pair: only a pair turns a node's bar span into seconds.
#[test]
fn a_bar_span_is_settled_by_a_whole_tempo_pair_and_by_nothing_less() {
    let whole = composition(&[
        ("bpm", "120\n"),
        ("meter", "4/4\n"),
        ("hit-1b", "sin(2*pi*220*t)\n"),
        ("master", "@hit-1b\n"),
    ]);
    let graph = prepared(&whole).expect("a tempo settles the span");
    let held = config_for(&graph, "hit-1b", None, None, None).expect("a settled span");
    assert_eq!(
        held.horizon.end_secs, 2.0,
        "one bar of four beats at 120 bpm"
    );

    for half in [
        composition(&[
            ("bpm", "120\n"),
            ("hit-1b", "sin(t)\n"),
            ("master", "@hit-1b\n"),
        ]),
        composition(&[
            ("meter", "4/4\n"),
            ("hit-1b", "sin(t)\n"),
            ("master", "@hit-1b\n"),
        ]),
        composition(&[
            ("bpm", "0\n"),
            ("meter", "4/4\n"),
            ("hit-1b", "sin(t)\n"),
            ("master", "@hit-1b\n"),
        ]),
    ] {
        assert!(
            matches!(prepared(&half), Err(CliError::BadTempo(_))),
            "half a tempo settles nothing"
        );
    }
}

fn asking<'a>(held: &'a sva_ast::Composition, target: &'a str) -> Job<'a> {
    Job {
        target: Some(target),
        ..Job::over(held)
    }
}

/// `--help` promises 24 for a node this composition does not define. A target that is only a
/// name used to become the synthetic `probe` node, and the free variable inside it was
/// reported as a bad composition rather than as a target nothing answers for.
#[test]
fn a_missing_target_through_settle_is_not_found() {
    let held = composition(&[("master", "sin(2*pi*220*t)\n")]);
    for missing in ["nosuchnode", "drums/kikc", "drums/kikc(gain=1)"] {
        let Err(refused) = execute(asking(&held, missing)) else {
            panic!("`{missing}` names no node, so nothing renders");
        };
        assert_eq!(refused.code(), "not_found", "{missing}: {refused:?}");
        assert_eq!(refused.exit_code(), 24, "{missing}");
        assert!(refused.message().contains(missing), "{}", refused.message());
    }

    for math in [
        "t",
        "440",
        "A4",
        "2*t",
        "sin(2*pi*220*t)",
        "master",
        "sum(k, 1, 3, sin(2*pi*110*k*t))",
    ] {
        assert!(
            execute(asking(&held, math)).is_ok(),
            "`{math}` reads as a value on its own"
        );
    }
}

/// A target that parses is math, whatever it is spelled like, so what it leaves unbound is the
/// engine's own refusal and what it fails to parse is the parser's. Neither is `not_found`:
/// answering 24 there sends a caller looking for a node it never named.
#[test]
fn a_target_that_is_not_a_path_answers_off_the_expression_it_is() {
    let held = composition(&[("master", "sin(2*pi*220*t)\n")]);
    for (text, code) in [
        ("2^3", "validation_error"),
        ("sin(q*t)", "validation_error"),
        ("sum(k, 1, 3, k*", "validation_error"),
    ] {
        let Err(refused) = execute(asking(&held, text)) else {
            panic!("`{text}` names no node and renders nothing");
        };
        assert_eq!(refused.code(), code, "{text}: {refused:?}");
    }

    let Err(refused) = execute(asking(&held, "2^3")) else {
        panic!("`^` has no token, so nothing renders");
    };
    assert!(
        refused.message().contains('^'),
        "the parser's own message names what it met: {}",
        refused.message()
    );
    let Err(refused) = execute(asking(&held, "sin(q*t)")) else {
        panic!("`q` is bound nowhere, so nothing renders");
    };
    assert!(
        refused.message().contains('q'),
        "the engine's own message names the free variable: {}",
        refused.message()
    );
}
