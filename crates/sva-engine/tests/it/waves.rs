// Concern: proves a named waveform is a function of its fundamental's phase, delayed or modulated whole | Non-concern: a written sum's own series (refs.rs) | IO: (a composition) -> samples

use crate::fixtures::graph_of;
use sva_engine::{RenderConfig, Tier, render};

const HALF_LSB: f64 = 1.0 / (1u64 << 24) as f64;

fn plane(files: &[(&str, &str)], root: &str, secs: f64) -> Vec<f64> {
    let g = graph_of(root, files);
    let held = render(
        &g,
        root,
        RenderConfig::seconds(44_100, secs),
        &Tier::default(),
    )
    .expect("a waveform renders");
    let id = held.id(root).expect("the root");
    held.output(id).expect("its samples").plane(0).to_vec()
}

fn peak(samples: &[f64]) -> f64 {
    samples.iter().fold(0.0f64, |held, s| held.max(s.abs()))
}

/// `pi/10` at 441 Hz delays the wave by exactly five steps at 44.1 kHz.
#[test]
fn a_constant_phase_delays_the_whole_wave() {
    for name in ["saw", "square", "triangle"] {
        let plain = plane(&[("plain", &format!("{name}(441)\n"))], "plain", 0.06);
        let phased = plane(
            &[("phased", &format!("{name}(441, pi/10)\n"))],
            "phased",
            0.05,
        );
        for (i, s) in phased.iter().enumerate() {
            let want = plain[i + 5];
            assert!(
                (s - want).abs() <= HALF_LSB,
                "{name} sample {i}: {s} against the plain wave's {want}"
            );
        }
    }
}

#[test]
fn a_quarter_turn_keeps_the_saw_s_peak() {
    let plain = plane(&[("plain", "saw(100)\n")], "plain", 0.05);
    let turned = plane(&[("turned", "saw(100, pi/2)\n")], "turned", 0.05);
    let (a, b) = (peak(&plain), peak(&turned));
    assert!(
        (a - b).abs() < 0.02 * a,
        "a delayed saw peaks as a saw: {b} against {a}"
    );
}
