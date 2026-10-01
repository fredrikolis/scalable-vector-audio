// Concern: states what a target sampling its one ref read, `sample(@x([a, b]))`, answers | Non-concern: `sample` inside a node | IO: (target) -> a reading, its source, memory's lookups

use sva_core::{Asked, CliError, Edge, Job, Out, Output, PROBE, asked, call, execute};
use sva_engine::{Source, Tier};

const RAMP: &str = "crop(tanh(4*(2*t - 1)), 0s, 1s)\n";
const NOTE: &str = "f0 = 220\ncrop(tanh(4*sin(2*pi*f0*t)), 0s, 4s)\n";

fn composition() -> sva_ast::Composition {
    [("n", RAMP), ("note", NOTE)].into_iter().collect()
}

fn pitch() -> Asked {
    asked(&call("pitch").expect("a call")).expect("a reading")
}

fn pitched(target: &str, tier: &Tier) -> Result<sva_engine::Answer, CliError> {
    let held = composition();
    let asking = [pitch()];
    let job = Job {
        asked: &asking,
        ..Job::over(&held, target)
    };
    execute(job, tier)?.answer(PROBE, asking[0].representation)
}

#[test]
fn a_plain_read_of_a_cropped_closed_form_refuses_exact_pitch() {
    let Err(refused) = pitched("@n([0, 1s])", &Tier::default()) else {
        panic!("a cropped term has no exact pitch");
    };
    let CliError::Engine(engine) = &refused else {
        panic!("the engine refuses it: {refused:?}");
    };
    assert_eq!(engine.code(), "cast.left_algebra", "{refused:?}");
}

/// A reader samples the one ref it reads and measures it over that ref's interval.
#[test]
fn a_sampled_read_measures_the_pitch_a_plain_one_refuses() {
    let answer = pitched("sample(@n([0, 1s]))", &Tier::default()).expect("measured");
    assert_eq!(answer.source, Source::Measured);
    let Output::Pitch(frames) = answer.value else {
        panic!("pitch answers frames");
    };
    assert_eq!(frames.len(), 20, "one frame per 50ms across the second");
}

#[test]
fn a_sampled_held_note_reads_its_binds_and_measures_their_pitch() {
    let answer = pitched("sample(@note([0, 4s], f0=261.6))", &Tier::default()).expect("measured");
    assert_eq!(answer.source, Source::Measured);
    let Output::Pitch(frames) = answer.value else {
        panic!("pitch answers frames");
    };
    assert_eq!(frames.len(), 80, "one frame per 50ms across four seconds");
    let lowest = frames[40]
        .notes
        .iter()
        .map(|n| n.hz)
        .fold(f64::INFINITY, f64::min);
    assert!((lowest - 261.6).abs() < 5.0, "the bound f0: {lowest}");
}

#[test]
fn a_sampled_read_leaves_memory_what_the_plain_read_answers_from() {
    let tier = Tier::default();
    let held = composition();
    let dropped = Job {
        out: Out::Dropped,
        ..Job::over(&held, "sample(@n([0, 1s]))")
    };
    execute(dropped, &tier).expect("the sampled read renders");
    let plain = execute(Job::over(&held, "@n([0, 1s])"), &tier).expect("the plain read renders");
    let stats = plain.render.cache_stats.expect("stats");
    assert_eq!(stats.computed(), 0, "{stats:#?}");
}

#[test]
fn a_sampled_ref_read_keeps_its_interval_and_binds() {
    let held = sva_core::target("sample(@note([1s, inf), f0=C4))").expect("an open interval");
    assert_eq!(held.expr, "sample(@note(t, f0=C4))");
    assert_eq!(held.interval, Some((Edge::Secs(1.0), Edge::Inf)));
    let bare = sva_core::target("sample(@note)").expect("no interval");
    assert_eq!((bare.expr.as_str(), bare.interval), ("sample(@note)", None));
}

#[test]
fn only_one_sampled_ref_read_holds_an_interval() {
    for refused in [
        "sample(@n(t) + @n([0, 1s]))",
        "sample(@n([0, 1s]) * 0.5)",
        "sample(@n([0, 1s])) * 0.5",
        "sample(sample(@n([0, 1s])))",
        "fourier(@n([0, 1s]))",
        "sample(@n([0, 1s]), 2)",
        "sample(@n([0, 1s])) + sample(@n(t))",
    ] {
        let Err(why) = sva_core::target(refused) else {
            panic!("`{refused}` refuses");
        };
        assert!(matches!(why, CliError::Usage(_)), "`{refused}`: {why:?}");
    }
}
