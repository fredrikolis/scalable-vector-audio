// Concern: states what the shared pipeline answers a front end handing it a target and a condition | Non-concern: what a render sounds like | IO: (text) -> a target, a condition, roots

use sva_core::{CliError, Edge, Job, execute, prepared, roots_of, target, until};
use sva_engine::{Cmp, Term, Until};

fn composition(files: &[(&str, &str)]) -> sva_ast::Composition {
    files.iter().copied().collect()
}

/// The target's own ref reads the interval; the expression reads it as `t`.
#[test]
fn a_target_splits_into_its_expression_and_the_interval_its_ref_reads() {
    let held = target("@piano([0, 2b], f0=C4, vel=0.5)").expect("an interval");
    assert_eq!(held.expr, "@piano(t, f0=C4, vel=0.5)");
    assert_eq!(held.interval, Some((Edge::Secs(0.0), Edge::Bars(2.0))));
    for (open, start) in [("@a([1s,))", 1.0), ("@a([500ms, inf))", 0.5)] {
        let held = target(open).expect("an open interval");
        assert_eq!(
            held.interval,
            Some((Edge::Secs(start), Edge::Inf)),
            "{open}"
        );
    }
    let held = target("@a([0, 48000sp])").expect("samples");
    assert_eq!(
        held.interval,
        Some((Edge::Secs(0.0), Edge::Samples(48_000.0)))
    );
    assert_eq!(target("@a*0.5").expect("no interval").interval, None);
}

#[test]
fn an_interval_anywhere_but_the_targets_own_ref_or_without_units_refuses() {
    for refused in [
        "@a(t) + @b([0, 1s])",
        "@a([0, 1s]) * 0.5",
        "@a([2, 3s])",
        "@a([inf, 1s])",
        "@a([0, 1db])",
    ] {
        assert!(
            matches!(target(refused), Err(CliError::Usage(_))),
            "`{refused}` refuses"
        );
    }
}

#[test]
fn a_condition_reads_comparisons_over_time_and_level() {
    let joined = until(
        "t > 2s or envelope(t) < -60db and t >= 1b",
        44_100,
        Some(2.0),
    )
    .expect("a joined condition");
    let Until::Any(left, right) = joined else {
        panic!("`or` binds loosest");
    };
    assert_eq!(*left, Until::Holds(Term::Time, Cmp::Gt, Term::Number(2.0)));
    let Until::All(level, bars) = *right else {
        panic!("`and` binds tighter");
    };
    assert_eq!(
        *level,
        Until::Holds(Term::Envelope, Cmp::Lt, Term::Number(0.001))
    );
    assert_eq!(*bars, Until::Holds(Term::Time, Cmp::Ge, Term::Number(2.0)));
    for refused in [
        "t > envelope(t)",
        "max(envelope([t, inf))) < -96db",
        "envelope(t - 50ms) < -60db",
        "envelope([t, inf)) < 0.1",
        "t",
        "t > 1s and",
    ] {
        assert!(
            matches!(until(refused, 44_100, None), Err(CliError::Usage(_))),
            "`{refused}` refuses"
        );
    }
    assert!(matches!(
        until("t > 2b", 44_100, None),
        Err(CliError::BadTempo(_))
    ));
}

/// A level compares with a level in `db`: a bare number beside one has no unit to be read
/// in, and the refusal points at it.
#[test]
fn a_level_compared_with_a_bare_number_refuses_where_it_is_written() {
    for (text, at) in [
        ("envelope(t) < -60", "`-60` (at bytes 14..17)"),
        ("t > 1s or 0.001 > envelope(t)", "`0.001` (at bytes 10..15)"),
    ] {
        let Err(CliError::Usage(why)) = until(text, 44_100, None) else {
            panic!("`{text}` compares a level with no unit");
        };
        assert!(why.contains(at), "{why}");
    }
}

fn asking<'a>(held: &'a sva_ast::Composition, target: &'a str) -> Job<'a> {
    Job::over(held, target)
}

#[test]
fn an_interval_that_holds_no_sample_or_names_bars_with_no_tempo_refuses() {
    let held = composition(&[("tone", "crop(sin(2*pi*220*t), 0s, 1s)\n")]);
    assert!(execute(asking(&held, "@tone([0, 0.5s])")).is_ok());
    assert!(matches!(
        execute(asking(&held, "@tone([1s, 0.5s])")),
        Err(CliError::Usage(_))
    ));
    assert!(matches!(
        execute(asking(&held, "@tone([0, 1b])")),
        Err(CliError::BadTempo(_))
    ));
}

/// What an expression reads are the roots a load pulls, and every reserved variable besides.
#[test]
fn the_roots_of_a_target_are_what_its_math_reads() {
    let source = composition(&[("master", "@kick\n"), ("kick", "sin(2*pi*60*t)\n")]);
    let holds = |target, name: &str| {
        roots_of(&source, target)
            .unwrap()
            .contains(&name.to_string())
    };
    assert!(holds("@kick*0.5", "kick"), "argv math pulls what it reads");
    assert!(
        holds("sin(t)", "bpm"),
        "and every reserved variable, which no ref walk reaches"
    );
}

/// `bpm` and `meter` come as a pair: only a pair turns a bar into seconds.
#[test]
fn a_bar_span_is_settled_by_a_whole_tempo_pair_and_by_nothing_less() {
    let whole = composition(&[
        ("bpm", "120\n"),
        ("meter", "4/4\n"),
        ("hit-1b", "sin(2*pi*220*t)\n"),
        ("master", "@hit-1b\n"),
    ]);
    let graph = prepared(&whole).expect("a tempo settles the span");
    assert_eq!(graph.seconds_per_bar(), Some(2.0), "four beats at 120 bpm");

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

/// A refusal names the target as the caller wrote it, never the node the pipeline defined it as.
#[test]
fn a_refusal_names_the_target_as_written() {
    let held = composition(&[("tone", "sin(2*pi*220*t)\n")]);
    let Err(refused) = execute(asking(&held, "@tone")) else {
        panic!("nothing ends an endless tone");
    };
    let message = refused.message();
    assert!(message.contains("(@tone)"), "{message}");
    assert!(!message.contains("probe"), "{message}");
}

/// A target that parses answers off the expression it is: what it leaves unbound is the
/// engine's own refusal, what it fails to parse the parser's.
#[test]
fn a_target_answers_off_the_expression_it_is() {
    let held = composition(&[("master", "crop(sin(2*pi*220*t), 0s, 1s)\n")]);
    for (text, code) in [
        ("2^3", "validation_error"),
        ("sin(q*t)", "validation_error"),
        ("sum(k, 1, 3, k*", "validation_error"),
    ] {
        let Err(refused) = execute(asking(&held, text)) else {
            panic!("`{text}` renders nothing");
        };
        assert_eq!(refused.code(), code, "{text}: {refused:?}");
    }
    let Err(refused) = execute(asking(&held, "sin(q*t)")) else {
        panic!("`q` is bound nowhere, so nothing renders");
    };
    assert!(
        refused.message().contains('q'),
        "the engine's own message names the free variable: {}",
        refused.message()
    );
    assert!(execute(asking(&held, "@master")).is_ok());
}
