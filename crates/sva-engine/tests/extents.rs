// Concern: proves every node is computed over its own support met with its readers' demand, whatever reads it | Non-concern: where a range ends (stream.rs) | IO: (a composition, a range) -> samples

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Range, RenderConfig, render};
use sva_samples::LATTICE_8K;

const RATE: u32 = LATTICE_8K.lattice_hz;

fn composition() -> Graph {
    graph_of(
        "extents",
        &[
            ("tone", "0.5*sin(2*pi*440*t)\n"),
            ("held", "sample(@tone)\n"),
            ("short", "sample(crop(@tone, 0s, 0.5s))\n"),
            ("late", "@short(t - 1s)\n"),
            ("early", "@short(t + 0.25s)\n"),
            ("heard_late", "@held(t - 0.5s)\n"),
            ("said_late", "@tone(t - 0.5s)\n"),
            (
                "echo",
                "sample(crop(0.5*sin(2*pi*440*t), 0s, 0.2s)) + 0.5*self(t - 0.25s)\n",
            ),
            (
                "ring",
                "lowpass(sample(crop(0.5*sin(2*pi*220*t), 0s, 0.05s)), 220, q=30)\n",
            ),
            ("whole", "istft(stft(@held, window=256sp, hop=64sp))\n"),
            ("smoothed", "lp(crop(4, 0s, 2s), cutoff=8)\n"),
            ("steps", "0.5 + 0.5*rand(t - t % 0.125, seed=17)\n"),
            (
                "drift",
                "0.5*(1 - cos(2*pi*0)) + 0.5*lp(sample(@steps), cutoff=55)\n",
            ),
            ("drift_late", "@drift(t - 0.5s)\n"),
        ],
    )
}

fn over(g: &Graph, target: &str, start: i64, end: i64) -> Vec<f64> {
    let config = RenderConfig {
        range: Range {
            start: Some(start),
            end: Some(end),
        },
        ..RenderConfig::at(RATE).under(LATTICE_8K)
    };
    let held = render(g, target, config, None).unwrap_or_else(|e| panic!("{target}: {e}"));
    held.output(held.root).expect("the root").plane(0).to_vec()
}

fn secs(n: f64) -> i64 {
    (n * f64::from(RATE)) as i64
}

/// Each read takes the samples its source holds there, never silence past a window.
#[test]
fn a_shifted_ref_reads_its_source_where_that_source_sounds() {
    let g = composition();
    let short = over(&g, "short", 0, secs(0.5));
    assert!(short.iter().any(|v| *v != 0.0), "silence tests nothing");
    let late = over(&g, "late", 0, secs(2.0));
    let before = &late[..secs(1.0) as usize];
    assert!(before.iter().all(|v| *v == 0.0), "silent before it starts");
    let heard = &late[secs(1.0) as usize..secs(1.5) as usize];
    assert_eq!(heard, &short[..], "a second late, the source's own samples");
    let after = &late[secs(1.5) as usize..];
    assert!(after.iter().all(|v| *v == 0.0), "silent after it ends");
    let early = over(&g, "early", 0, secs(0.5));
    let ahead = &early[..secs(0.25) as usize];
    assert_eq!(
        ahead,
        &short[secs(0.25) as usize..],
        "a read ahead reads ahead"
    );
}

#[test]
fn a_crop_inside_a_loop_or_under_a_filter_keeps_the_tail() {
    let g = composition();
    let echo = over(&g, "echo", 0, secs(4.5));
    let echoes = &echo[secs(0.25) as usize..secs(0.45) as usize];
    assert!(
        echoes.iter().any(|v| v.abs() > 0.1),
        "the first echo sounds"
    );
    assert!(
        echo[secs(4.0) as usize..].iter().any(|v| *v != 0.0),
        "the sixteenth echo sounds"
    );
    let ring = over(&g, "ring", 0, secs(0.1));
    let tail = &ring[secs(0.05) as usize..];
    assert!(
        tail.iter().any(|v| v.abs() > 1e-3),
        "the ring sounds past the crop"
    );
}

#[test]
fn a_start_past_zero_keeps_the_state_its_history_left() {
    let g = composition();
    for target in ["echo", "ring"] {
        let whole = over(&g, target, 0, secs(1.0));
        let late = over(&g, target, secs(0.3), secs(1.0));
        assert!(late.iter().any(|v| *v != 0.0), "{target}: silence");
        assert_eq!(late[..], whole[secs(0.3) as usize..], "{target}");
    }
}

#[test]
fn a_sampled_form_read_at_a_shift_is_its_closed_form_read_there() {
    let g = composition();
    let heard = over(&g, "heard_late", 0, secs(1.0));
    let said = over(&g, "said_late", 0, secs(1.0));
    let before = &heard[..secs(0.5) as usize];
    assert!(
        before.iter().any(|v| v.abs() > 0.1),
        "not silence where no window started"
    );
    let worst = heard
        .iter()
        .zip(&said)
        .fold(0.0f64, |held, (a, b)| held.max((a - b).abs()));
    assert!(worst < 1e-9, "sample() moved the value by {worst}");
}

/// A range may start before 0, and then holds what one from 0 holds where they meet.
#[test]
fn a_range_from_before_zero_reads_a_filtered_crop_where_it_sounds() {
    let g = composition();
    let from_zero = over(&g, "smoothed", 0, secs(1.0));
    assert!(from_zero.iter().any(|v| *v != 0.0), "silence tests nothing");
    let early = over(&g, "smoothed", -secs(0.5), secs(1.0));
    assert!(early[..secs(0.5) as usize].iter().all(|v| *v == 0.0));
    assert_eq!(early[secs(0.5) as usize..], from_zero[..]);
}

/// A filter of an input with no start starts at t = 0 however it is read, earlier or late,
/// here where it runs inside its reader's own program.
#[test]
fn a_filter_starts_where_its_support_does_whatever_reads_it() {
    let g = composition();
    let from_zero = over(&g, "drift", 0, secs(1.0));
    assert!(from_zero.iter().any(|v| *v != 0.0), "silence tests nothing");
    let early = over(&g, "drift", -secs(0.5), secs(1.0));
    let late = over(&g, "drift_late", 0, secs(1.5));
    for (read, what) in [
        (early, "read from before 0"),
        (late, "read half a second late"),
    ] {
        let (before, after) = read.split_at(secs(0.5) as usize);
        assert!(
            before.iter().all(|v| *v == 0.0),
            "{what}: sounds before t = 0"
        );
        assert_eq!(after, &from_zero[..], "{what}");
    }
}

/// A short-time transform reads its input whole, so an input with no end refuses.
#[test]
fn a_transform_of_an_endless_input_refuses() {
    let config = RenderConfig::seconds(RATE, 0.1).under(LATTICE_8K);
    let Err(refused) = render(&composition(), "whole", config, None) else {
        panic!("a transform of an endless input rendered");
    };
    assert_eq!(refused.code(), "engine.unbounded_extent", "{refused}");
}
