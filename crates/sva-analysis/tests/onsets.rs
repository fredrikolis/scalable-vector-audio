// Concern: proves `onsets` refuses a tempo grid no onset can be placed on | Non-concern: where a grid comes from (sva-core's tempo.rs) | IO: (samples, tempo) -> Onsets or a refusal

use sva_analysis::{Request, TempoGrid, run};

fn asking(tempo: Option<TempoGrid>) -> Request<'static> {
    Request {
        samples: &[0.0; 4410],
        sample_rate: 44_100.0,
        start_secs: 0.0,
        frame_secs: 0.05,
        tempo,
        envelope: &[],
        stereo: None,
        against: None,
        gated: false,
        input_envelope: None,
    }
}

#[test]
fn a_zero_bar_length_refuses() {
    let none = TempoGrid {
        seconds_per_bar: 0.0,
        beats_per_bar: 4.0,
    };
    let err = run("onsets", &asking(Some(none))).unwrap_err();
    assert!(err.0.contains("bar"), "{}", err.0);

    let real = TempoGrid {
        seconds_per_bar: 2.0,
        ..none
    };
    assert!(run("onsets", &asking(Some(real))).is_ok());
    assert!(run("onsets", &asking(None)).is_ok());
}

#[test]
fn a_bar_shorter_than_a_sample_refuses() {
    let tiny = TempoGrid {
        seconds_per_bar: 1e-300,
        beats_per_bar: 4.0,
    };
    let err = run("onsets", &asking(Some(tiny))).unwrap_err();
    assert!(
        err.0.contains("1e-300"),
        "the bar length is named: {}",
        err.0
    );

    // 4410 samples at 44_100 Hz is 0.1s, so one sample is the shortest bar that still fits.
    let shortest = TempoGrid {
        seconds_per_bar: 1.0 / 44_100.0,
        ..tiny
    };
    assert!(run("onsets", &asking(Some(shortest))).is_ok());
}
