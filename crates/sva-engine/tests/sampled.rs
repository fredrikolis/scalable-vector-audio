// Concern: proves a discrete node renders at the observation's rate, accumulator and all | Non-concern: any solver's own grid (sva-samples) | IO: (a composition, rate) -> a Buffer

mod fixtures;

use fixtures::graph_of;
use sva_engine::{RenderConfig, render};

/// A solver is a family indexed by the rate, each run as long as its own grid.
#[test]
fn an_fd_node_renders_at_the_observation_rate() {
    let g = graph_of("fd", &[("body", "chaigne_askenfelt(261.63)\n")]);
    for rate in [22_050u32, 44_100] {
        let held = render(&g, "body", RenderConfig::seconds(rate, 0.02), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        let root = held.id("body").expect("the root");
        let buffer = held.buffer(root).expect("a rendered solver");
        assert_eq!(buffer.rate, rate);
        assert_eq!(buffer.len(), (rate as f64 * 0.02).round() as usize);
        assert!(
            buffer.plane(0).iter().any(|s| s.abs() > 0.0),
            "{rate}: the string sounded"
        );
    }
}

/// A one-step recurrence over a constant is a running sum.
#[test]
fn a_one_step_accumulator_renders() {
    let g = graph_of(
        "accumulator",
        &[("acc", "sample(0.25 + 0*t) + self(t - 1sp)\n")],
    );
    let held = render(&g, "acc", RenderConfig::seconds(1_000, 0.01), None).expect("a recurrence");
    let root = held.id("acc").expect("the root");
    let buffer = held.buffer(root).expect("a rendered loop");
    assert_eq!(buffer.len(), 10);
    for (i, held) in buffer.plane(0).iter().enumerate() {
        let want = 0.25 * (i + 1) as f64;
        assert!(
            (held - want).abs() < 1e-12,
            "sample {i}: {held} against {want}"
        );
    }
}

/// A sampled filter is the biquad recurrence, not a closed form's response.
#[test]
fn a_filter_over_samples_runs_on_the_grid() {
    let g = graph_of(
        "biquad",
        &[("voice", "lowpass(sample(sin(2*pi*4000*t)), 200, 0.7)\n")],
    );
    let held = render(&g, "voice", RenderConfig::seconds(44_100, 0.05), None).expect("a biquad");
    let root = held.id("voice").expect("the root");
    let buffer = held.buffer(root).expect("a rendered filter");
    let tail: f64 = buffer.plane(0)[2_000..]
        .iter()
        .map(|s| s * s)
        .sum::<f64>()
        .sqrt();
    assert!(tail < 0.2, "4 kHz under a 200 Hz lowpass is gone: {tail}");
}

/// `sp` as a duration is `1/R` seconds, so a coefficient written with it tracks the rate
/// and the time constant it states stays put in seconds.
#[test]
fn a_one_pole_smoother_written_with_sp_renders_at_two_rates() {
    let g = graph_of(
        "smoother",
        &[(
            "smooth",
            "1 - exp(0 - 1sp/0.01) + self(t - 1sp)*exp(0 - 1sp/0.01)\n",
        )],
    );
    let level = |rate: u32| -> f64 {
        let held = render(&g, "smooth", RenderConfig::seconds(rate, 0.02), None)
            .unwrap_or_else(|e| panic!("{rate}: {e}"));
        let root = held.id("smooth").expect("the root");
        held.buffer(root)
            .expect("a rendered loop")
            .at(0, rate as usize / 100)
    };
    let settled = 1.0 - (-1.0f64).exp();
    let (low, high) = (level(44_100), level(96_000));
    for (rate, held) in [(44_100, low), (96_000, high)] {
        let apart = 20.0 * (held / settled).log10();
        assert!(
            apart.abs() < 0.5,
            "{rate}: one time constant reads {held}, not {settled}"
        );
    }
    let apart = 20.0 * (low / high).log10();
    assert!(
        apart.abs() < 0.5,
        "the two rates state one time constant: {low} against {high}"
    );
}

/// A cutoff that moves is a recurrence redesigned per sample, so the same tone is stopped
/// while the corner sits below it and passed once the corner has swept past.
#[test]
fn a_swept_cutoff_on_samples_renders() {
    let g = graph_of(
        "swept",
        &[(
            "filtered",
            "lowpass(sample(sin(2*pi*1000*t)), cutoff=150 + 7800*t, q=0.707)\n",
        )],
    );
    let held =
        render(&g, "filtered", RenderConfig::seconds(44_100, 0.5), None).expect("a swept filter");
    let root = held.id("filtered").expect("the root");
    let buffer = held.buffer(root).expect("a rendered recurrence");
    let peak = |from: f64, to: f64| {
        let span = (from * 44_100.0) as usize..(to * 44_100.0) as usize;
        span.map(|i| buffer.at(0, i).abs()).fold(0.0f64, f64::max)
    };
    let (stopped, passed) = (peak(0.006, 0.03), peak(0.45, 0.5));
    assert!(
        20.0 * (stopped / passed).log10() < -12.0,
        "the corner below the tone stops it: {stopped} against {passed}"
    );
    assert!(
        (20.0 * passed.log10()).abs() < 0.5,
        "the corner above the tone passes it: {passed}"
    );
}

/// No block sizes a sampled loop: a delay-`d` read lands on `i - d`, and `written`
/// refounded at every `i` founds it. Longer than one step, it reads silence until then.
#[test]
fn a_self_read_is_founded_sample_by_sample_without_a_block() {
    let g = graph_of(
        "three-step",
        &[("acc", "sample(1 + 0*t) + 0.5*self(t - 3sp)\n")],
    );
    let held = render(&g, "acc", RenderConfig::seconds(1_000, 0.01), None).expect("a recurrence");
    let root = held.id("acc").expect("the root");
    let buffer = held.buffer(root).expect("a rendered loop");
    assert_eq!(buffer.len(), 10);

    let mut want = [0.0f64; 10];
    for i in 0..10 {
        want[i] = 1.0 + 0.5 * if i >= 3 { want[i - 3] } else { 0.0 };
    }
    for (i, held) in buffer.plane(0).iter().enumerate() {
        assert!(
            (held - want[i]).abs() < 1e-12,
            "sample {i}: {held} against {}",
            want[i]
        );
    }
    assert_eq!(buffer.at(0, 2), 1.0, "before the first founded read");
    assert_eq!(buffer.at(0, 3), 1.5, "the first founded read");
}

fn plane(g: &sva_ast::Graph, node: &str) -> Vec<f64> {
    let held = render(g, node, RenderConfig::seconds(8_000, 0.05), None)
        .unwrap_or_else(|e| panic!("{node}: {e}"));
    let id = held.id(node).unwrap_or_else(|| panic!("{node} typed"));
    held.buffer(id).expect("a rendered node").plane(0).to_vec()
}

/// The bug this closes: a sampled operand's crop kept its hard edges and dropped both
/// shoulders, so the same signal cropped the same way sounded two different windows.
#[test]
fn a_sampled_crop_takes_the_closed_forms_window() {
    let g = graph_of(
        "crop-shoulders",
        &[
            (
                "closed",
                "crop(sin(2*pi*220*t), 10ms, 40ms, rise=5ms, fall=10ms)\n",
            ),
            (
                "sampled",
                "crop(sample(sin(2*pi*220*t)), 10ms, 40ms, rise=5ms, fall=10ms)\n",
            ),
            ("hard", "crop(sample(sin(2*pi*220*t)), 10ms, 40ms)\n"),
        ],
    );
    let (closed, sampled) = (plane(&g, "closed"), plane(&g, "sampled"));
    assert_eq!(closed.len(), sampled.len());
    for (i, (c, s)) in closed.iter().zip(&sampled).enumerate() {
        assert!(
            (c - s).abs() < 1e-9,
            "sample {i}: the closed form reads {c}, the sampled crop {s}"
        );
        let inside = (80..320).contains(&i);
        assert!(inside || *s == 0.0, "sample {i} lies outside [10ms, 40ms)");
    }
    let hard = plane(&g, "hard");
    let first = (0.01 * 8_000.0) as usize + 1;
    assert!(
        sampled[first].abs() < hard[first].abs(),
        "the rise opens the window gradually: {} against {}",
        sampled[first],
        hard[first]
    );
}

#[test]
fn a_sampled_crop_refuses_the_shoulders_a_closed_form_refuses() {
    let g = graph_of(
        "crop-shoulders-refused",
        &[(
            "body",
            "crop(sample(sin(2*pi*220*t)), 10ms, 20ms, rise=6ms, fall=6ms)\n",
        )],
    );
    let Err(refused) = render(&g, "body", RenderConfig::seconds(8_000, 0.05), None) else {
        panic!("two shoulders longer than their window render nothing");
    };
    assert!(
        refused.to_string().contains("shoulder")
            || format!("{refused:?}").contains("bad_crop_shoulder"),
        "{refused:?}"
    );
}
