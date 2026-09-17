// Concern: proves a grid written against the reserved key sounds in whatever key that node names | Non-concern: the grid's row arithmetic (render_integration.rs) | IO: (a key) -> its lines

mod helpers;

use std::path::PathBuf;

use helpers::{put, scratch};
use sva_core::run;
use sva_engine::{Output, Representation};

const GRID: &str = "; Models: two steps on the key | Neglects: dynamics | IO: t -> amplitude \
                    | Tags: grid\n@voice(t, f0=@variables/key)\n@voice(t, \
                    f0=@variables/key*7st)\n";

fn composition(key: &str) -> PathBuf {
    let dir = scratch(&format!("key-{key}"));
    put(
        &dir,
        "variables/bpm",
        "; Models: the tempo | Neglects: swing | IO: none -> bpm | Tags: variable\n120\n",
    );
    put(
        &dir,
        "variables/meter",
        "; Models: the meter | Neglects: swing | IO: none -> beats | Tags: variable\n4/4\n",
    );
    put(
        &dir,
        "variables/key",
        &format!("; Models: the tonic | Neglects: mode | IO: none -> Hz | Tags: variable\n{key}\n"),
    );
    put(
        &dir,
        "voice",
        "; Models: one sine voice | Neglects: envelope | IO: (t, f0) -> amplitude | Tags: voice\nsin(2*pi*f0*t)\n",
    );
    put(&dir, "phrase-1b", GRID);
    put(
        &dir,
        "master",
        "; Models: the phrase | Neglects: nothing | IO: t -> amplitude | Tags: root\n@phrase-1b\n",
    );
    dir
}

fn lines(key: &str) -> Vec<f64> {
    let rendered = run(&composition(key)).unwrap_or_else(|e| panic!("{key}: {e}"));
    let Output::Lines(lines) = rendered
        .answer("master", Representation::Lines)
        .unwrap_or_else(|e| panic!("{key}: {e}"))
        .value
    else {
        panic!("{key}: expected a line list");
    };
    let mut hz: Vec<f64> = lines
        .iter()
        .map(|l| l.hz.abs())
        .filter(|h| *h > 1.0)
        .collect();
    hz.sort_by(f64::total_cmp);
    hz.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    hz
}

#[test]
fn a_key_agnostic_grid_renders_in_two_keys() {
    let low = lines("D3");
    let high = lines("A3");
    assert_eq!(low.len(), 2, "two steps, two lines: {low:?}");
    assert_eq!(high.len(), 2, "two steps, two lines: {high:?}");
    let fifth = 2f64.powf(7.0 / 12.0);
    for (a, b) in low.iter().zip(&high) {
        assert!(
            (b / a - fifth).abs() < 1e-12,
            "the same grid a fifth up: {a} then {b}"
        );
    }
}
