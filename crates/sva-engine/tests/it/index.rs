// Concern: proves an index read reads the lattice sample its integer names, whole or streamed, or refuses | Non-concern: a read at an instant (refs.rs) | IO: (a composition) -> samples or a refusal

use crate::fixtures::graph_of;
use sva_ast::{Graph, PerBar};
use sva_engine::{Range, RenderConfig, Stream, StreamConfig, render};
use sva_samples::LATTICE_8K;

const RATE: u32 = LATTICE_8K.lattice_hz;

fn bits(samples: &[f64]) -> Vec<u64> {
    assert!(samples.iter().any(|v| *v != 0.0), "silence tests nothing");
    samples.iter().map(|v| v.to_bits()).collect()
}

fn whole(g: &Graph, target: &str, config: &RenderConfig) -> Vec<f64> {
    let held = render(g, target, config.clone(), None).unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Vec<f64> {
    let config = StreamConfig {
        block,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(samples as i64),
            },
            ..RenderConfig::at(RATE).under(LATTICE_8K)
        },
    };
    let at = sva_ast::parse_expr(&format!("@{target}")).expect("a ref");
    let mut stream = Stream::open(g, &at, config, None).unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::with_capacity(samples + block);
    while out.len() < samples {
        match stream.next_block().unwrap_or_else(|e| panic!("{e}")) {
            Some(block) => out.extend_from_slice(block.plane(0)),
            None => break,
        }
    }
    out.truncate(samples);
    out
}

fn at_128_bpm(name: &str, files: &[(&str, &str)]) -> Graph {
    let mut g = graph_of(name, files);
    g.resolve_bar_spans(PerBar {
        seconds: 240.0,
        per: 128.0,
    });
    g
}

const ONSET: &str = "crop(sample(sin(2*pi*220*t)), 0s, 0.05s)";

#[test]
fn a_loop_one_index_back_is_the_loop_one_step_back_bit_for_bit() {
    let g = graph_of(
        "index-loop",
        &[
            ("indexed", &format!("{ONSET} + 0.5*self[idx(t) - 1]\n")),
            ("stepped", &format!("{ONSET} + 0.5*self(t - 1sp)\n")),
        ],
    );
    let config = RenderConfig::seconds(RATE, 0.1).under(LATTICE_8K);
    assert_eq!(
        bits(&whole(&g, "indexed", &config)),
        bits(&whole(&g, "stepped", &config))
    );
}

/// Half a bar at 128 bpm is 41343.75 samples of 44.1 kHz: the index rounds it to 41344 and
/// reads that sample, where the instant itself is read through the kernel.
#[test]
fn an_index_half_a_bar_back_reads_the_nearest_sample_with_no_kernel() {
    let g = at_128_bpm(
        "index-bar",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 1s), cutoff=900, q=0.8)\n",
            ),
            ("indexed", "@x[idx(t - 0.5b)]\n"),
            ("nearest", "@x(t - 41344sp)\n"),
            ("between", "@x(t - 0.5b)\n"),
        ],
    );
    let config = RenderConfig::seconds(44_100, 1.2);
    assert_eq!(
        bits(&whole(&g, "indexed", &config)),
        bits(&whole(&g, "nearest", &config))
    );
    let rows = |target: &str| {
        let held = render(&g, target, config.clone(), None).expect("a render");
        held.reconstructions()
            .into_iter()
            .filter(|r| r.node == target)
            .count()
    };
    assert_eq!(rows("indexed"), 0, "an index read takes no kernel");
    assert_eq!(rows("between"), 1, "the instant between samples does");
}

#[test]
fn a_time_or_a_fraction_in_an_index_refuses_at_typing_naming_idx() {
    for (node, code) in [
        ("@x[t - 0.5b]", "type.non_integer_index"),
        ("@x[3/2]", "type.non_integer_index"),
        (
            "sample(sin(2*pi*220*t)) + 0.5*self[t - 1sp]",
            "type.non_integer_index",
        ),
        ("@x(idx(t)*1s)", "type.index_outside_read"),
    ] {
        let g = at_128_bpm(
            "index-refused",
            &[
                ("x", "sample(sin(2*pi*220*t))\n"),
                ("node", &format!("{node}\n")),
            ],
        );
        let Err(refused) = sva_engine::types(&g, "node") else {
            panic!("`{node}` must refuse");
        };
        assert_eq!(refused.code(), code, "{node}: {refused}");
        if code == "type.non_integer_index" {
            assert!(refused.to_string().contains("use idx("), "{refused}");
        }
    }
}

/// An integer the engine reads no one map for types, and its render refuses.
#[test]
fn an_integer_no_rounded_line_spells_types_and_refuses_to_render() {
    let g = graph_of(
        "index-unread",
        &[
            ("x", "sample(sin(2*pi*220*t))\n"),
            ("node", "@x[idx(t) + idx(t - 1s)]\n"),
        ],
    );
    assert!(sva_engine::types(&g, "node").is_ok());
    let config = RenderConfig::seconds(RATE, 0.1).under(LATTICE_8K);
    let Err(refused) = render(&g, "node", config, None) else {
        panic!("two rounded lines are no one map");
    };
    assert_eq!(refused.code(), "engine.unreadable_index", "{refused}");
}

/// `floor` and `ceil` pick the samples either side of an index that ties; a negated index
/// rounds the mirrored way.
#[test]
fn floor_ceil_and_a_negated_index_read_the_samples_either_side() {
    let g = graph_of(
        "index-round",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.05s), cutoff=900)\n",
            ),
            ("floored", "@x[idx(t - 2.5sp, floor)]\n"),
            ("ceiled", "@x[idx(t - 2.5sp, ceil)]\n"),
            ("mirrored", "@x[0 - idx(2.5sp - t, floor)]\n"),
            ("three", "@x(t - 3sp)\n"),
            ("two", "@x(t - 2sp)\n"),
        ],
    );
    let config = RenderConfig::seconds(RATE, 0.1).under(LATTICE_8K);
    let read = |target: &str| bits(&whole(&g, target, &config));
    assert_eq!(read("floored"), read("three"));
    assert_eq!(read("ceiled"), read("two"));
    assert_eq!(read("mirrored"), read("two"));
}

/// A read whose index ties between two samples rounds to the even one, so its delay moves
/// sample to sample; a stream reads each where a whole render does.
#[test]
fn index_reads_stream_the_samples_a_whole_render_writes_bit_for_bit() {
    let g = graph_of(
        "index-stream",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.05s), cutoff=900)\n",
            ),
            ("shifted", "@x[idx(t - 0.01s) - 3]\n"),
            ("tied", "@x[idx(t - 2.5sp)]\n"),
            ("looped", &format!("{ONSET} + 0.5*self[idx(t) - 1]\n")),
            (
                "alternating",
                &format!("{ONSET} + 0.5*self[idx(t - 1.5sp)]\n"),
            ),
        ],
    );
    let samples = 800;
    let config = RenderConfig::seconds(RATE, samples as f64 / f64::from(RATE)).under(LATTICE_8K);
    for target in ["shifted", "tied", "looped", "alternating"] {
        let want = bits(&whole(&g, target, &config));
        for block in [1, 64, 333] {
            assert_eq!(
                bits(&streamed(&g, target, block, samples)),
                want,
                "{target} in blocks of {block}"
            );
        }
    }
}
