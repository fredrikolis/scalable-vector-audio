// Concern: proves an index read reads the sample its integer names, whole or streamed | Non-concern: a read at an instant (refs.rs) | IO: (a composition) -> samples or a refusal

use crate::fixtures::{Now, graph_of, next};
use sva_ast::{Graph, PerBar};
use sva_engine::{Range, RenderConfig, Stream, StreamConfig, Tier, render};

const RATE: u32 = 8_000;

fn bits(samples: &[f64]) -> Vec<u64> {
    assert!(samples.iter().any(|v| *v != 0.0), "silence tests nothing");
    samples.iter().map(|v| v.to_bits()).collect()
}

fn whole(g: &Graph, target: &str, config: &RenderConfig) -> Vec<f64> {
    let held = render(g, target, config.clone(), &Tier::default())
        .unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

fn streamed(g: &Graph, target: &str, block: usize, samples: usize) -> Vec<f64> {
    let config = StreamConfig {
        block,
        channels: None,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(samples as i64),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let at = sva_ast::parse_expr(&format!("@{target}")).expect("a ref");
    let mut stream = Stream::open(g, &at, config, &Tier::default())
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::with_capacity(samples + block);
    while out.len() < samples {
        match next(&mut stream).unwrap_or_else(|e| panic!("{e}")) {
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

/// Half a bar at 128 bpm is 41343.75 samples of 44.1 kHz: the index rounds it to 41344 and
/// reads that sample.
#[test]
fn an_index_half_a_bar_back_reads_the_nearest_sample() {
    let g = at_128_bpm(
        "index-bar",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 1s), cutoff=900, q=0.8)\n",
            ),
            ("indexed", "@x[idx(t - 0.5b)]\n"),
            ("nearest", "@x(t - 41344sp)\n"),
        ],
    );
    let config = RenderConfig::seconds(44_100, 1.2);
    assert_eq!(
        bits(&whole(&g, "indexed", &config)),
        bits(&whole(&g, "nearest", &config))
    );
}

/// `x[idx(w)]` with `w` moving reads, at each sample, the stored step nearest `w`.
#[test]
fn an_index_of_a_moving_time_reads_the_step_nearest_it() {
    let g = graph_of(
        "index-moving",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.05s), cutoff=900)\n",
            ),
            ("wobbled", "@x[idx(t - 0.005s - 0.002s*sin(2*pi*5*t))]\n"),
        ],
    );
    let config = RenderConfig::seconds(RATE, 0.1);
    let (x, wobbled) = (whole(&g, "x", &config), whole(&g, "wobbled", &config));
    reads_nearest(&x, &wobbled, |t| {
        t - 0.005 - 0.002 * (2.0 * std::f64::consts::PI * 5.0 * t).sin()
    });
}

/// Each sample of `read` is the step of `x` nearest the time `at` names, either one at a tie.
fn reads_nearest(x: &[f64], read: &[f64], at: impl Fn(f64) -> f64) {
    let rate = f64::from(RATE);
    bits(read);
    for (n, v) in read.iter().enumerate() {
        let at = at(n as f64 / rate) * rate;
        let step = |k: f64| match k < 0.0 {
            true => 0.0,
            false => x.get(k as usize).copied().unwrap_or(0.0),
        };
        let sides = [step(at.floor()), step(at.ceil())];
        match (at - at.floor() - 0.5).abs() < 1e-6 {
            true => assert!(sides.contains(v), "sample {n} reads between {sides:?}"),
            false => assert_eq!(v.to_bits(), step(at.round()).to_bits(), "sample {n}"),
        }
    }
}

/// `idx` of any time reads the step nearest it: a finite or infinite sum, a remainder, and a
/// `min`, `max` or `sin` around an unbounded form.
#[test]
fn an_index_of_any_time_reads_the_step_nearest_it() {
    let tail = 0.001 / (std::f64::consts::E - 1.0);
    let reads: [(&str, &dyn Fn(f64) -> f64); 7] = [
        ("t - sum(k, 1, 2, 0.01s)", &|t| t - 0.02),
        ("t - sum(k, 1, 2, 0.01s*step(t - k*0.01s))", &|t| {
            t - 0.01 * [0.01, 0.02].iter().filter(|d| t >= **d).count() as f64
        }),
        ("t - sum(k, 1, inf, 0.001s*exp(0 - k))", &|t| t - tail),
        // Four steps of 8 kHz, whole on the grid; under 0.1s, `t % 0.3s` is `t`.
        ("t - t % 0.5ms", &|t| {
            ((t * f64::from(RATE)).round() as i64 / 4 * 4) as f64 / f64::from(RATE)
        }),
        ("t - t % 0.06ms", &|t| t - t % 0.000_06),
        ("t - 0.1s*sin(t % 0.3s)", &|t| t - 0.1 * t.sin()),
        ("t - max(min(exp(t), 0.003), 0)", &|t| t - 0.003),
    ];
    let mut files = vec![(
        "x".to_string(),
        "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.05s), cutoff=900)\n".to_string(),
    )];
    for (k, (time, _)) in reads.iter().enumerate() {
        files.push((format!("read{k}"), format!("@x[idx({time})]\n")));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let g = graph_of("index-any", &files);
    let config = RenderConfig::seconds(RATE, 0.1);
    let x = whole(&g, "x", &config);
    for (k, (time, at)) in reads.iter().enumerate() {
        let read = render(&g, &format!("read{k}"), config.clone(), &Tier::default())
            .unwrap_or_else(|e| panic!("idx({time}): {e}"));
        let id = read.id(&format!("read{k}")).expect("the root");
        reads_nearest(&x, read.output(id).expect("a buffer").plane(0), at);
        let samples = x.len();
        assert_eq!(
            bits(&streamed(&g, &format!("read{k}"), 64, samples)),
            bits(&whole(&g, &format!("read{k}"), &config)),
            "idx({time}) streamed"
        );
    }
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

/// A whole render holds what an index no bound holds reads ahead, as it does behind.
#[test]
fn a_whole_render_reads_ahead_where_no_bound_holds_the_index() {
    let g = graph_of(
        "index-ahead",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 0.05s), cutoff=900)\n",
            ),
            ("ahead", "@x[idx(t + 0.001s*sin(2*pi*50*t))]\n"),
        ],
    );
    let x = whole(&g, "x", &RenderConfig::seconds(RATE, 0.2));
    let read = whole(&g, "ahead", &RenderConfig::seconds(RATE, 0.1));
    reads_nearest(&x, &read, |t| {
        t + 0.001 * (2.0 * std::f64::consts::PI * 50.0 * t).sin()
    });
}

/// Any integer is an index: two rounded lines summed read the sample they sum to.
#[test]
fn a_sum_of_two_indices_reads_the_sample_it_names() {
    let g = graph_of(
        "index-summed",
        &[
            ("x", "sample(sin(2*pi*220*t))\n"),
            ("node", "@x[idx(t) + idx(t - 1s)]\n"),
        ],
    );
    let config = RenderConfig::seconds(RATE, 0.1);
    let rate = f64::from(RATE);
    for (n, v) in whole(&g, "node", &config).iter().enumerate() {
        let at = (2.0 * n as f64 - rate) / rate;
        let want = (2.0 * std::f64::consts::PI * 220.0 * at).sin();
        assert!((v - want).abs() < 1e-9, "sample {n}: {v} against {want}");
    }
}

/// A read whose reach a constant or a clamp by constants bounds holds only that much of its
/// source at any block; one no bound holds keeps all of it, and still plays.
#[test]
fn an_index_holds_of_its_source_only_what_a_bounded_reach_needs() {
    let g = graph_of(
        "index-held",
        &[
            (
                "x",
                "lowpass(crop(sample(sin(2*pi*220*t)), 0s, 2s), cutoff=900)\n",
            ),
            ("shifted", "@x[idx(t - 1ms) - 3]\n"),
            (
                "clamped",
                "@x[idx(t - min(max(0.002s*sin(2*pi*t), 0s), 0.002s))]\n",
            ),
            ("unbounded", "@x[idx(t - 0.1s*exp(sin(t % 0.3s)))]\n"),
        ],
    );
    let samples = 2 * RATE as usize;
    let held = |target: &str| {
        let config = StreamConfig {
            block: 64,
            channels: None,
            render: RenderConfig {
                range: Range {
                    start: Some(0),
                    end: Some(samples as i64),
                },
                ..RenderConfig::at(RATE)
            },
        };
        let at = sva_ast::parse_expr(&format!("@{target}")).expect("a ref");
        let mut stream = Stream::open(&g, &at, config, &Tier::default())
            .now()
            .unwrap_or_else(|e| panic!("{e}"));
        let mut most = 0;
        while next(&mut stream)
            .unwrap_or_else(|e| panic!("{e}"))
            .is_some()
        {
            most = most.max(stream.held_bytes());
        }
        most
    };
    let whole_source = samples * size_of::<f64>();
    for target in ["shifted", "clamped"] {
        assert!(
            held(target) < whole_source / 4,
            "{target} holds {}",
            held(target)
        );
    }
    assert!(held("unbounded") >= whole_source, "{}", held("unbounded"));
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
    let config = RenderConfig::seconds(RATE, 0.1);
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
    let config = RenderConfig::seconds(RATE, samples as f64 / f64::from(RATE));
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

/// An index taken off another node's samples holds no bound, so the read asks every sample of
/// a signal that never starts or ends: refused by name, whole or streamed, never computed.
#[test]
fn an_unbounded_index_into_a_signal_with_no_ends_refuses() {
    let g = graph_of(
        "endless-index",
        &[
            ("rom", "crop(sample(sin(2*pi*100*t)), 0s, 0.01s)\n"),
            ("ramp", "crop(sample(t/1sp), 0s, 10s)\n"),
            ("lfo", "sample(2ms + 0.2ms*sin(2*pi*5*t))\n"),
            (
                "wrap",
                "w = 0\nw[idx(t - 0.01s*@ramp[idx(t*100*1sp, floor)], floor)]\n",
            ),
            ("mdly", "x = 0\nx[idx(t) - idx(@lfo(t), floor) - 2]\n"),
            ("top", "crop(@mdly(t, x=@wrap(t, w=@rom(t))), 0s, 0.05s)\n"),
        ],
    );
    let refused = render(&g, "top", RenderConfig::at(48_000), &Tier::default())
        .err()
        .expect("every sample of `wrap` is asked");
    let said = refused.to_string();
    assert!(
        said.contains("wrap") && said.contains("never starts or ends"),
        "{said}"
    );
    let config = StreamConfig {
        block: 64,
        channels: None,
        render: RenderConfig::at(48_000),
    };
    let at = sva_ast::parse_expr("@top").expect("a ref");
    let opened = Stream::open(&g, &at, config, &Tier::default()).now();
    let refused = opened.and_then(|mut stream| next(&mut stream)).err();
    let said = refused.expect("a stream refuses too").to_string();
    assert!(
        said.contains("wrap") && said.contains("never starts or ends"),
        "{said}"
    );
}

/// A moving delay's index, `t` less a time that holds no `t`, reaches a bounded span back, so
/// a signal that never starts or ends is asked only there: the bits a cropped one renders.
#[test]
fn a_moving_delay_of_a_signal_with_no_ends_reads_only_where_its_index_reaches() {
    let files = |x: &str| {
        graph_of(
            "moving-delay",
            &[
                ("rom", "crop(sample(sin(2*pi*100*t)), 0s, 0.01s)\n"),
                ("ramp", "crop(sample(t/1sp), 0s, 10s)\n"),
                (
                    "wrap",
                    "w = 0\nw[idx(t - 0.01s*@ramp[idx(t*100*1sp, floor)], floor)]\n",
                ),
                (
                    "mdly",
                    "x = 0\nx[idx(t) - idx(2ms + 0.2ms*sin(2*pi*5*t), floor) - 2]\n",
                ),
                ("top", &format!("crop(@mdly(t, x={x}), 0s, 0.05s)\n")),
            ],
        )
    };
    let config = RenderConfig::at(48_000);
    let endless = whole(&files("@wrap(t, w=@rom(t))"), "top", &config);
    let cropped = whole(
        &files("@wrap(t, w=@rom(t))*crop(1, 0s, 1s)"),
        "top",
        &config,
    );
    assert_eq!(bits(&endless), bits(&cropped));
}
