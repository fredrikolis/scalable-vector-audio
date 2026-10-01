// Concern: proves a fixed filter's ringing past its input's end is cut where its pole bound stays under the prune level | Non-concern: deriving the bound | IO: (a composition) -> samples, the cut

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
            ("swept", "lowpass(sample(@src), 800hz + 400*sin(2*pi*t))\n"),
            ("open", "lowpass(sample(sin(2*pi*440*t)), 300hz)\n"),
            ("echo", "sample(@src) + 0.5*self[idx(t) - 400]\n"),
            ("held", "sample(@src) + self[idx(t) - 400]\n"),
        ],
    )
}

const SHAPES: [&str; 10] = [
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
/// the render states the level it cut at.
#[test]
fn a_fixed_filter_over_a_cropped_input_ends() {
    let g = rings();
    for shape in SHAPES {
        let bare = render(&g, shape, RenderConfig::at(RATE));
        let pruned = bare.labels[&bare.root]
            .pruned
            .clone()
            .expect("a stated level");
        assert_eq!(pruned.db, -120.0, "{shape}");
        let samples = plane(&bare);
        let cut = samples.len() as i64;
        assert!(
            pruned
                .cuts
                .iter()
                .any(|(node, at)| node == shape && *at == cut),
            "{shape} ends at its own cut: {cut}, {:?}",
            pruned.cuts
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
/// there, and only zeros after.
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
        assert!(
            long[bare.len()..].iter().all(|v| v.to_bits() == 0),
            "{shape}"
        );
    }
}

/// What the cut zeroes was already under -120 dBFS: rendered with a far lower level, every
/// sample from the -120 dBFS cut on is under it.
#[test]
fn what_the_cut_zeroes_is_under_the_level() {
    let g = rings();
    let deep = RenderConfig {
        profile: Profile {
            prune_db: -300.0,
            ..PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 20.0)
    };
    for shape in SHAPES {
        let cut = plane(&render(&g, shape, RenderConfig::at(RATE))).len();
        let unpruned = plane(&render(&g, shape, deep.clone()));
        let loudest = unpruned[cut..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(loudest < 1e-6, "{shape}: {loudest} past {cut}");
        assert!(unpruned[..cut].iter().any(|v| v.abs() > 0.0), "{shape}");
    }
}

/// A pole close to the unit circle sums to a large gain, so the rounding its feedback adds
/// is bounded as it falls with the ringing, not by the loudest sample: every shape at a low
/// cutoff over a cropped input still ends past it, and is zero only once under the level.
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
            prune_db: -300.0,
            ..PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 30.0)
    };
    for (name, body) in &files {
        let bare = plane(&render(&g, name, RenderConfig::at(RATE)));
        assert!(bare.len() > RATE as usize, "{body} rings past its input");
        let unpruned = plane(&render(&g, name, deep.clone()));
        assert!(
            unpruned.len() > bare.len(),
            "{body} ends within the long render"
        );
        let loudest = unpruned[bare.len()..]
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

/// A term that ended leaves its past in a filter that read the note sum, so only that filter
/// loses its bound: one reading no note keeps its cut, as the whole render of it does.
#[test]
fn a_retired_term_leaves_a_filter_that_reads_no_note_its_cut() {
    use crate::fixtures::{Now, added, next};
    use sva_engine::{Stream, StreamConfig};
    let g = rings();
    let expr = |text: &str| sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("{}", e.message));
    let config = StreamConfig {
        block: 256,
        channels: None,
        render: RenderConfig::at(RATE),
    };
    let stream = Stream::open(&g, &expr("@notes + @lowpass"), config, &Tier::default()).now();
    let stream = std::cell::RefCell::new(stream.expect("opens"));
    let add = |text: &str| {
        added(&stream, &g, &expr(text), &Tier::default())
            .now()
            .unwrap_or_else(|e| panic!("{e}"))
    };
    let cut = || {
        let pruned = stream.borrow().pruned();
        pruned.cuts.into_iter().find(|(node, _)| node == "lowpass")
    };
    let whole = render(&g, "lowpass", RenderConfig::seconds(RATE, 1.0));
    let cut_whole = whole.labels[&whole.root].pruned.clone().expect("a level");
    let cut_whole = cut_whole
        .cuts
        .into_iter()
        .find(|(node, _)| node == "lowpass");
    assert!(cut_whole.is_some(), "the filter rings out");
    add("crop(sin(2*pi*500*t), 0s, 0.01s)");
    assert_eq!(cut(), cut_whole);
    for _ in 0..8 {
        next(&mut stream.borrow_mut()).expect("a block");
    }
    assert_eq!(stream.borrow().counts().terms, 0, "the note ended");
    add("crop(sin(2*pi*700*t), 0.5s, 0.6s)");
    assert_eq!(
        cut(),
        cut_whole,
        "the note that ended left the filter its cut"
    );
}
