// Concern: proves the three ways to lowpass one chord end where FORMAT 7.3 says they end | Non-concern: the filter response itself (sva-formula) | IO: (a composition) -> a reading or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Ask, EngineError, Output, RenderConfig, Representation, Source, answer, render};

/// 261.63, 329.63 and 392 Hz: a C major triad, one line each.
const CHORD: &str = "sin(2*pi*261.63*t) + sin(2*pi*329.63*t) + sin(2*pi*392*t)\n";

fn lines(name: &str, body: &str) -> Vec<(f64, f64)> {
    let g = graph_of(name, &[("chord", CHORD), ("voiced", body)]);
    let config = RenderConfig::seconds(44_100, 1.0).asking(vec![Ask {
        node: "voiced".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "voiced", config, None).unwrap_or_else(|e| panic!("{name}: {e}"));
    let id = held.id("voiced").expect("the root");
    let found = answer(&held, id, Representation::Lines).expect("lines");
    assert_eq!(found.source, Source::Exact, "{name}");
    assert_eq!(found.profile, "psychoacoustic-v1", "{name}");
    let Output::Lines(lines) = found.value else {
        panic!("{name}: expected a line list");
    };
    let mut out: Vec<(f64, f64)> = lines
        .iter()
        .filter(|l| l.hz > 0.0)
        .map(|l| (l.hz, 2.0 * l.amp.abs()))
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// FORMAT 7.3's first two rows are one value.
#[test]
fn the_written_filter_and_the_two_casts_around_it_agree() {
    let direct = lines("direct", "lowpass(@chord, 300, 0.7)\n");
    let stated = lines("stated", "ifourier(lowpass(fourier(@chord), 300, 0.7))\n");
    assert_eq!(direct.len(), 3, "three lines in, three lines out");
    assert_eq!(stated.len(), 3);
    for (a, b) in direct.iter().zip(&stated) {
        assert!((a.0 - b.0).abs() < 1e-9, "{a:?} against {b:?}");
        assert!((a.1 - b.1).abs() < 1e-12, "{a:?} against {b:?}");
    }
}

/// Every line sits at the response's own value, to the profile's 0.5 dB.
#[test]
fn the_triad_under_a_300_hz_lowpass_is_placed_at_the_response() {
    let found = lines("kept", "ifourier(lowpass(fourier(@chord), 300, 0.7))\n");
    assert_eq!(found.len(), 3, "three lines in, three lines out: {found:?}");
    let response = |hz: f64| {
        let r = hz / 300.0;
        let (real, imaginary) = (1.0 - r * r, r / 0.7);
        1.0 / (real * real + imaginary * imaginary).sqrt()
    };
    for (hz, amplitude) in &found {
        let want = response(*hz);
        let off = 20.0 * (amplitude / want).log10();
        assert!(off.abs() < 0.5, "{hz} Hz sits {off} dB off its response");
    }
    let root = found[0];
    assert!((root.0 - 261.63).abs() < 1e-9, "{root:?}");
    assert!(
        found[1].1 < root.1 && found[2].1 < found[1].1,
        "a lowpass falls with frequency: {found:?}"
    );
}

/// The third row: a quotient built from `f` has no dual in A.
#[test]
fn the_quotient_row_refuses_and_names_the_cast() {
    let g = graph_of(
        "quotient",
        &[
            ("chord", CHORD),
            ("voiced", "ifourier(@chord/(1 + pow(f/300, 8)))\n"),
        ],
    );
    let Err(refused) = render(&g, "voiced", RenderConfig::seconds(44_100, 1.0), None) else {
        panic!("a quotient of a law leaves A");
    };
    let EngineError::Refused(d) = &refused else {
        panic!("expected a written refusal, got {refused:?}");
    };
    assert_eq!(d.code, "cast.left_algebra");
    assert!(d.help.contains("sample("), "{}", d.help);
}

/// A crop a second wide puts the pole row's reference a second from the origin, where the
/// old prefactor overflowed: the response it holds is the one FORMAT 10.1 states.
#[test]
fn a_lowpass_over_a_second_long_crop_holds_its_response() {
    let g = graph_of(
        "long-crop",
        &[
            ("chord", CHORD),
            ("voiced", "lp(crop(sin(2*pi*100*t), 0s, 1s), cutoff=200)\n"),
        ],
    );
    let held = render(&g, "voiced", RenderConfig::seconds(44_100, 0.5), None)
        .expect("a cropped pair through a pole");
    let id = held.id("voiced").expect("the root");
    let buffer = held.buffer(id).expect("a collapsed law");
    let peak = (4_410..buffer.len())
        .map(|i| buffer.at(0, i).abs())
        .fold(0.0f64, f64::max);
    let want = 1.0 / (1.0f64 + 0.25).sqrt();
    assert!(
        (20.0 * (peak / want).log10()).abs() < 0.5,
        "the one-pole response at half its corner is {peak}, not {want}"
    );
}
