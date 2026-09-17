// Concern: what a tempo decides about a render's length and its bar-resolved timing | Non-concern: the readings a render answers (the parent suite) | IO: (a composition) -> a duration or a refusal

use crate::helpers::{fixture, plane, put, scratch, secs};
use sva_ast::Dir;
use sva_cli::CliError;
use sva_core::{Job, PROBE, execute, probe, run};
use sva_engine::DiskCache;

/// `bpm` reaches the samples only by rewriting a grid's row shifts, so a tempo change must
/// re-render exactly the bar-timed nodes and leave the rest byte-identical.
#[test]
fn a_bpm_change_re_renders_bar_resolved_timing_and_nothing_else() {
    let dir = scratch("bpm-cache");
    put(&dir, "meter", "4/4\n");
    put(&dir, "kick", "sin(2*pi*55*t)*exp(0 - 30*t)\n");
    put(&dir, "tone", "sin(2*pi*440*t)*0.1\n");
    put(&dir, "pattern-1b", "@kick*1.0\n@kick*0.5\n\n@kick*0.7\n");
    put(&dir, "master", "crop(@pattern-1b(t), 0s, 3s) + @tone*0.2\n");

    let store = scratch("bpm-store");
    let cache = DiskCache::at(&store).storing_everything();
    let job = || {
        execute(Job {
            cache: Some(&cache),
            ..Job::over(&Dir::at(&dir))
        })
        .expect("the fixture renders")
    };
    let at = |node: &'static str| {
        execute(Job {
            target: Some(node),
            cache: Some(&cache),
            ..Job::over(&Dir::at(&dir))
        })
        .expect("the node renders")
    };
    put(&dir, "bpm", "120\n");
    let fast = job();
    let fast_tone = at("tone");
    let fast_pattern = at("pattern-1b");
    put(&dir, "bpm", "100\n");
    let slow = job();
    let slow_tone = at("tone");
    let slow_pattern = at("pattern-1b");

    assert_eq!(secs(&fast), 3.0);
    assert_eq!(secs(&slow), 3.0, "same window, slower grid");
    assert_eq!(
        plane(&slow_tone, "tone"),
        plane(&fast_tone, "tone"),
        "a node no bar reaches is the same signal"
    );
    assert_ne!(
        plane(&slow_pattern, "pattern-1b"),
        plane(&fast_pattern, "pattern-1b"),
        "and the hits really did move"
    );
}

/// A `b` literal is grid units: without a tempo it refuses rather than defaulting.
#[test]
fn a_bar_literal_needs_a_tempo_and_reaches_a_probe_expression_too() {
    let dir = scratch("bar-literal");
    put(&dir, "master", "crop(@tone(t - 0.5b), 0s, 2b)\n");
    put(&dir, "tone", "sin(2*pi*A4*t)\n");
    assert!(
        matches!(run(&dir), Err(CliError::BadTempo(_))),
        "a bar literal with no bpm/meter must refuse"
    );

    put(&dir, "variables/bpm", "120\n");
    put(&dir, "variables/meter", "4/4\n");
    assert_eq!(secs(&run(&dir).unwrap()), 4.0, "2 bars at 120bpm 4/4");

    let bars = probe(&dir, "crop(1, 0s, 1b)").unwrap();
    let seconds = probe(&dir, "crop(1, 0s, 2s)").unwrap();
    assert_eq!(
        plane(&bars, PROBE),
        plane(&seconds, PROBE),
        "1b IS 2s at this tempo"
    );
}

/// Globals belong in `variables/`; the root spelling is transitional and loses to it.
#[test]
fn tempo_comes_from_variables_before_the_composition_root() {
    let dir = scratch("variables");
    put(&dir, "master", "concat(@loop-2b)\n");
    put(&dir, "loop-2b", "sin(2*pi*220*t)\n");
    put(&dir, "variables/bpm", "120\n");
    put(&dir, "variables/meter", "4/4\n");
    assert_eq!(secs(&run(&dir).unwrap()), 4.0, "two bars at 120");

    put(&dir, "bpm", "240\n");
    put(&dir, "meter", "4/4\n");
    assert_eq!(
        secs(&run(&dir).unwrap()),
        4.0,
        "variables/ wins while both spellings are present"
    );
}

#[test]
fn bpm_present_without_meter_is_a_clear_bad_tempo_error() {
    let dir = scratch("partial-tempo");
    put(&dir, "master", "sin(2*pi*220*t)\n");
    put(&dir, "bpm", "120\n");
    assert!(matches!(run(&dir), Err(CliError::BadTempo(_))));
}

/// A bar at 128 bpm is 82687.5 samples at 44.1 kHz and exactly 90000 at 48 kHz.
#[test]
fn a_bar_at_128_bpm_lands_on_a_sample_only_at_the_rate_that_divides_it() {
    let dir = fixture("bar-grid");
    for (hz, exact) in [(44_100u32, false), (48_000u32, true)] {
        let rendered = execute(Job {
            target: Some("hit-1b"),
            sample_rate: Some(hz),
            ..Job::over(&Dir::at(&dir))
        })
        .unwrap();
        assert_eq!(secs(&rendered), 1.875, "one bar at 128 bpm");
        let bar = secs(&rendered) * f64::from(hz);
        assert_eq!(bar.fract() == 0.0, exact, "{hz} Hz puts a bar at {bar}");
        assert_eq!(plane(&rendered, "hit-1b").len(), bar.round() as usize);
    }
}
