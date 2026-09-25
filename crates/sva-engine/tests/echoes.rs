// Concern: proves a closed loop's series lands each echo where its recurrence does | Non-concern: classifying the loop (loops.rs), running a recurrence (sampled.rs) | IO: (a composition) -> a Buffer

mod fixtures;

use std::f64::consts::TAU;

use fixtures::graph_of;
use sva_engine::{RenderConfig, render};

const RATE: u32 = 44_100;

fn rendered(files: &[(&str, &str)], root: &str, secs: f64) -> Vec<f64> {
    let g = graph_of(root, files);
    let held = render(&g, root, RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("{root}: {e}"));
    let id = held.id(root).expect("the root");
    held.buffer(id).expect("a buffer").plane(0).to_vec()
}

/// Echo `k` of a burst over `[l, r)` sounds over `[l + k*d, r + k*d)` at `g^k`.
fn echoes(t: f64, l: f64, r: f64, g: f64, d: f64, terms: i32) -> f64 {
    (0..terms)
        .map(|k| {
            let at = t - d * f64::from(k);
            match (l..r).contains(&at) {
                true => g.powi(k) * (TAU * 440.0 * at).sin(),
                false => 0.0,
            }
        })
        .sum()
}

fn assert_within(held: &[f64], want: &[f64], by: f64, what: &str) {
    assert_eq!(held.len(), want.len(), "{what}: one length");
    for (i, (a, b)) in held.iter().zip(want).enumerate() {
        assert!((a - b).abs() <= by, "{what}, sample {i}: {a} against {b}");
    }
}

fn assert_agree(held: &[f64], want: &[f64], what: &str) {
    assert_within(held, want, 1e-12, what);
}

/// Each echo's window moves with its delay, and the echoes the series drops sum to less than
/// half the output's least significant bit.
#[test]
fn a_cropped_burst_echoes_where_its_recurrence_does() {
    let secs = 5.0;
    let series = rendered(
        &[(
            "echo",
            "crop(sin(2*pi*440*t), 0.3s, 0.35s) + 0.35*self(t - 0.25s)\n",
        )],
        "echo",
        secs,
    );
    let sampled = rendered(
        &[(
            "echo",
            "sample(crop(sin(2*pi*440*t), 0.3s, 0.35s)) + 0.35*self(t - 0.25s)\n",
        )],
        "echo",
        secs,
    );
    assert_within(
        &series,
        &sampled,
        sva_samples::PSYCHOACOUSTIC_V1.half_lsb(),
        "the series against the recurrence",
    );
    let want: Vec<f64> = (0..series.len())
        .map(|i| echoes(i as f64 / f64::from(RATE), 0.3, 0.35, 0.35, 0.25, 17))
        .collect();
    assert_agree(&series, &want, "the series against its echoes");
}

#[test]
fn a_burst_read_by_ref_echoes_at_its_delay() {
    let secs = 0.6;
    let series = rendered(
        &[
            ("blip", "crop(sin(2*pi*440*t), 0s, 0.05s)\n"),
            ("echo", "@blip + 0.35*self(t - 0.25s)\n"),
        ],
        "echo",
        secs,
    );
    let want: Vec<f64> = (0..series.len())
        .map(|i| echoes(i as f64 / f64::from(RATE), 0.0, 0.05, 0.35, 0.25, 3))
        .collect();
    assert_agree(&series, &want, "the series against its echoes");
}

#[test]
fn a_delayed_read_of_an_echo_moves_every_window_once() {
    let secs = 1.1;
    let late = rendered(
        &[
            (
                "echo",
                "crop(sin(2*pi*440*t), 0.3s, 0.35s) + 0.35*self(t - 0.25s)\n",
            ),
            ("late", "@echo(t - 0.1s)\n"),
        ],
        "late",
        secs,
    );
    let want: Vec<f64> = (0..late.len())
        .map(|i| echoes(i as f64 / f64::from(RATE) - 0.1, 0.3, 0.35, 0.35, 0.25, 3))
        .collect();
    assert_agree(&late, &want, "the delayed series against its echoes");
}
