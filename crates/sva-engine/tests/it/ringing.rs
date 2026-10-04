// Concern: proves a fixed filter's ringing past its input's end is cut where its pole bound stays under the silence threshold | Non-concern: deriving the bound | IO: (a composition) -> samples, the cut

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{PSYCHOACOUSTIC_V1, Profile, Render, RenderConfig, Tier};

const RATE: u32 = 8_000;

fn rings() -> Graph {
    graph_of(
        "ringing",
        &[
            ("src", "crop(sin(2*pi*440*t), 0s, 0.25s)\n"),
            ("lp", "lp(sample(@src), 300hz)\n"),
            ("lowpass", "lowpass(sample(@src), 300hz)\n"),
            ("highpass", "highpass(sample(@src), 300hz)\n"),
            ("bandpass", "bandpass(sample(@src), 440hz, 20)\n"),
            ("notch", "notch(sample(@src), 440hz, 2)\n"),
            ("peaking", "peaking(sample(@src), 440hz, 2, 12)\n"),
            ("lowshelf", "lowshelf(sample(@src), 300hz, 0.7, -6)\n"),
            ("highshelf", "highshelf(sample(@src), 300hz, 0.7, 6)\n"),
            (
                "cascade",
                "highpass(lowpass(sample(@src), 2000hz), 100hz)\n",
            ),
            ("shaped", "crop(tanh(3*sin(2*pi*200*t)), 0s, 0.25s)\n"),
            ("saturated", "lowpass(sample(@shaped), 300hz)\n"),
            (
                "drawn",
                "lowpass(sample(crop(rand(t, seed=7), 0s, 0.25s)), 300hz)\n",
            ),
            ("tone", "sin(2*pi*440*t)\n"),
            (
                "warped",
                "lowpass(sample(crop(@tone(t + 0.001*sin(2*pi*5*t)), 0s, 0.25s)), 300hz)\n",
            ),
            ("buzz", "crop(saw(220, 0.3*sin(2*pi*5*t)), 0s, 0.25s)\n"),
            ("vibrato", "lowpass(sample(@buzz), 300hz)\n"),
            (
                "modulated",
                "lowpass(sample(crop(sum(k, 1, 30, sin(k*(2*pi*220*t + 0.3*sin(2*pi*5*t)))/k), \
                 0s, 0.25s)), 300hz)\n",
            ),
            ("swept", "lowpass(sample(@src), 800hz + 400*sin(2*pi*t))\n"),
            ("open", "lowpass(sample(sin(2*pi*440*t)), 300hz)\n"),
            ("echo", "sample(@src) + 0.5*self[idx(t) - 400]\n"),
            ("held", "sample(@src) + self[idx(t) - 400]\n"),
        ],
    )
}

const SHAPES: [&str; 14] = [
    "drawn",
    "warped",
    "vibrato",
    "modulated",
    "lp",
    "lowpass",
    "highpass",
    "bandpass",
    "notch",
    "peaking",
    "lowshelf",
    "highshelf",
    "cascade",
    "saturated",
];

fn render(g: &Graph, target: &str, config: RenderConfig) -> Render {
    sva_engine::render(g, target, config, &Tier::default())
        .unwrap_or_else(|e| panic!("{target}: {e}"))
}

fn plane(r: &Render) -> Vec<f64> {
    r.output(r.root).expect("the root").plane(0).to_vec()
}

/// Each shape over a cropped input ends where its own cut says, past the input's end, and
/// the render states the silence threshold it cut at.
#[test]
fn a_fixed_filter_over_a_cropped_input_ends() {
    let g = rings();
    for shape in SHAPES {
        let bare = render(&g, shape, RenderConfig::at(RATE));
        let cutting = bare.labels[&bare.root]
            .cutting_below_silence_threshold
            .clone()
            .expect("a stated silence threshold");
        assert_eq!(cutting.silence_threshold_dbfs, -120.0, "{shape}");
        let samples = plane(&bare);
        let cut = samples.len() as i64;
        assert!(
            cutting
                .treated_as_silent_from_sample
                .iter()
                .any(|(node, at)| node == shape && *at == cut),
            "{shape} ends at its own cut: {cut}, {:?}",
            cutting.treated_as_silent_from_sample
        );
        let input_end = RATE as usize / 4;
        assert!(cut as usize > input_end, "{shape} rings past its input");
        assert!(
            samples[input_end..].iter().any(|v| *v != 0.0),
            "{shape} rings on after its input"
        );
        assert!(cut < 20 * RATE as i64, "{shape} cut at {cut}");
    }
}

/// The cut changes nothing before it: a render over a long fixed range has the same bits
/// there, and only samples under the silence threshold after.
#[test]
fn the_bare_render_is_the_long_render_up_to_its_cut() {
    let g = rings();
    for shape in SHAPES {
        let bare = plane(&render(&g, shape, RenderConfig::at(RATE)));
        let long = plane(&render(&g, shape, RenderConfig::seconds(RATE, 20.0)));
        assert!(long.len() > bare.len(), "{shape}");
        for (n, (a, b)) in bare.iter().zip(&long).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "{shape} sample {n}");
        }
        let silence_threshold = PSYCHOACOUSTIC_V1.silence_threshold_amplitude();
        assert!(
            long[bare.len()..]
                .iter()
                .all(|v| v.abs() < silence_threshold),
            "{shape}"
        );
    }
}

/// What the cut zeroes was already under -120 dBFS: rendered with a far lower silence threshold, every
/// sample from the -120 dBFS cut on is under it.
#[test]
fn what_the_cut_zeroes_is_under_the_silence_threshold() {
    let g = rings();
    let deep = RenderConfig {
        profile: Profile {
            silence_threshold_dbfs: -300.0,
            ..PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 20.0)
    };
    for shape in SHAPES {
        let cut = plane(&render(&g, shape, RenderConfig::at(RATE))).len();
        let uncut = plane(&render(&g, shape, deep.clone()));
        let loudest = uncut[cut..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(loudest < 1e-6, "{shape}: {loudest} past {cut}");
        assert!(uncut[..cut].iter().any(|v| v.abs() > 0.0), "{shape}");
    }
}

/// A pole close to the unit circle sums to a large gain, so the rounding its feedback adds
/// is bounded as it falls with the ringing, not by the loudest sample: every shape at a low
/// cutoff over a cropped input still ends past it, and is zero only once under the silence threshold.
#[test]
fn a_fixed_filter_with_a_pole_near_one_ends() {
    const RATE: u32 = 44_100;
    let input = "sample(crop(sin(2*pi*440*t), 0s, 1s))";
    let shapes = [
        format!("highpass({input}, cutoff=100, q=0.7)"),
        format!("lowpass({input}, cutoff=1000)"),
        format!("lowpass({input}, cutoff=40, q=0.7)"),
        format!("bandpass({input}, 60hz, 4)"),
        format!("notch({input}, 50hz, 0.5)"),
        format!("lowshelf({input}, 60hz, 0.7, -6)"),
    ];
    let files: Vec<(String, String)> = shapes
        .iter()
        .enumerate()
        .map(|(k, body)| (format!("f{k}"), format!("{body}\n")))
        .collect();
    let named: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let g = graph_of("near-one", &named);
    let deep = RenderConfig {
        profile: Profile {
            silence_threshold_dbfs: -300.0,
            ..PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 30.0)
    };
    for (name, body) in &files {
        let bare = plane(&render(&g, name, RenderConfig::at(RATE)));
        assert!(bare.len() > RATE as usize, "{body} rings past its input");
        let uncut = plane(&render(&g, name, deep.clone()));
        assert!(
            uncut.len() > bare.len(),
            "{body} ends within the long render"
        );
        let loudest = uncut[bare.len()..]
            .iter()
            .fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(loudest < 1e-6, "{body}: {loudest} past {}", bare.len());
    }
}

/// A moving cutoff, an input that never ends, and a loop have no proven decay: each bare
/// render still refuses for want of an end.
#[test]
fn a_filter_or_loop_with_no_proven_decay_never_ends() {
    let g = rings();
    for target in ["swept", "open", "echo", "held"] {
        let refused = sva_engine::render(&g, target, RenderConfig::at(RATE), &Tier::default())
            .err()
            .unwrap_or_else(|| panic!("{target} has no end"));
        assert!(
            refused.to_string().contains("no end"),
            "{target}: {refused}"
        );
    }
}
