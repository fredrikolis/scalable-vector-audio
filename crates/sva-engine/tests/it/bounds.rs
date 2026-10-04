// Concern: proves where a bound ends an open render, the root uncut holds only samples under the silence threshold | Non-concern: where a cut lands (faded.rs) | IO: (a composition) -> cuts, samples

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{PSYCHOACOUSTIC_V1, Profile, Range, Render, RenderConfig, Tier};

const RATE: u32 = 8_000;

/// Products, sums, quotients, powers, folds and shifted, scaled and sampled reads of refs, over
/// decaying, held and unbounded operands; each node `n` beside `s_n`, its sampled reader.
const NODES: &[(&str, &str)] = &[
    ("wa", "crop(min(1, max(0, t)), 0s, 1s)"),
    ("f", "crop((@wa(t)*1000 + 0.01)/(@wa(t) + 0.01), 0s, 1s)"),
    ("p", "crop(@f(t)*@f(t) - 100, 0s, 1s)"),
    ("q", "crop(@p(t)*@f(t), 0s, 1s)"),
    ("q_inline", "crop((@f(t)*@f(t) - 100)*@f(t), 0s, 1s)"),
    ("d1", "sin(2*pi*200*t)*exp(-t/0.15)"),
    ("d2", "crop(sin(2*pi*90*t), 0s, 3s)*exp(-t/0.3)"),
    ("held", "0.5*sin(2*pi*300*t)"),
    ("prod", "@d1(t)*@d2(t)"),
    ("square", "@d1(t)*@d1(t) - @d2(t)"),
    ("sum", "@d1(t) + 0.5*@d1(t - 0.1s)"),
    ("shifted", "@d2(t - 0.5s)"),
    ("scaled", "0.25*@d1(2*t)"),
    ("sampled", "2*sample(@d1(t))*sample(@d2(t))"),
    ("quot", "@d1(t)/(2 + @d2(t))"),
    ("cube", "pow(@d1(t), 3)"),
    ("most", "max(@d1(t), @d2(t))"),
    ("least", "min(@d1(t), -@held(t))"),
    ("loud", "@f(t)*@d1(t)"),
    ("louder", "@q(t)*@d1(t) + @p(t)"),
    ("faded", "@held(t)*@d2(t) + @prod(t)"),
    ("gated", "sample(sin(2*pi*(200 + 900*t)*t))*step(t - 0.01s)"),
    ("opened", "@gated(t)*@d1(t)"),
];

fn graph() -> Graph {
    let mut files: Vec<(String, String)> = Vec::new();
    for (name, body) in NODES {
        files.push(((*name).to_string(), format!("{body}\n")));
        files.push((format!("s_{name}"), format!("sample(@{name}(t))\n")));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    graph_of("bounds", &files)
}

fn rendered(g: &Graph, target: &str, silence_threshold_dbfs: f64) -> Render {
    over(
        g,
        target,
        silence_threshold_dbfs,
        RenderConfig::seconds(RATE, 4.0),
    )
    .unwrap_or_else(|e| panic!("{target}: {e}"))
}

fn over(
    g: &Graph,
    target: &str,
    silence_threshold_dbfs: f64,
    config: RenderConfig,
) -> Result<Render, sva_engine::EngineError> {
    let config = RenderConfig {
        profile: Profile {
            silence_threshold_dbfs,
            ..PSYCHOACOUSTIC_V1
        },
        ..config
    };
    sva_engine::render(g, target, config, &Tier::default())
}

fn plane(r: &Render) -> Vec<f64> {
    r.output(r.root).expect("the root").plane(0).to_vec()
}

/// Every open render a bound ends holds: its root, rendered a second past the cut with nothing
/// cut, stays under the silence threshold from the cut on.
#[test]
fn every_cut_zeroes_only_samples_under_the_silence_threshold() {
    let g = graph();
    let silence_threshold = PSYCHOACOUSTIC_V1.silence_threshold_amplitude();
    let (mut checked, mut wrong) = (0, Vec::new());
    for (name, _) in NODES {
        let open = RenderConfig::at(RATE);
        let Ok(r) = over(&g, name, PSYCHOACOUSTIC_V1.silence_threshold_dbfs, open) else {
            continue;
        };
        let cutting = r.labels[&r.root]
            .cutting_below_silence_threshold
            .clone()
            .expect("a stated silence threshold");
        for (node, from) in cutting.treated_as_silent_from_sample {
            assert_eq!(node, *name, "only the root is cut");
            let reference = RenderConfig {
                range: Range {
                    start: Some(0),
                    end: Some(from.max(0) + i64::from(RATE)),
                },
                ..RenderConfig::at(RATE)
            };
            let truth = over(&g, name, f64::NEG_INFINITY, reference).expect("a render");
            let truth = plane(&truth);
            let from = usize::try_from(from.max(0)).expect("a sample");
            let peak = truth[from..].iter().fold(0.0f64, |m, v| m.max(v.abs()));
            if peak >= silence_threshold {
                wrong.push(format!("{name} cut from {from}, reaching {peak:e}"));
            }
            checked += 1;
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(checked > 0, "no render cut anything");
}

/// A product of refs bounds as the same product written inline: neither is cut.
#[test]
fn a_product_of_refs_renders_as_written_inline() {
    let g = graph();
    let (refs, inline) = (
        plane(&rendered(
            &g,
            "s_q",
            PSYCHOACOUSTIC_V1.silence_threshold_dbfs,
        )),
        plane(&rendered(
            &g,
            "s_q_inline",
            PSYCHOACOUSTIC_V1.silence_threshold_dbfs,
        )),
    );
    let peak = |x: &[f64]| x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(peak(&inline) > 9e8, "{}", peak(&inline));
    assert_eq!(refs.len(), inline.len());
    for (n, (a, b)) in refs.iter().zip(&inline).enumerate() {
        assert!((a - b).abs() <= 1e-9 * b.abs(), "sample {n}: {a} {b}");
    }
}

/// 1/44100 s written as the decimal its double prints lies past sample 1's instant: an open
/// render's bound holds the samples the crop does, of an atom sum's window or a written form's.
#[test]
fn a_bound_meets_a_crop_edge_tying_an_instant_as_the_grid_does() {
    let g = graph_of(
        "tie",
        &[
            ("click", "crop(1, 0s, 0.000022675736961451248s)\n"),
            ("bend", "crop(tanh(1000*t), 0s, 0.000022675736961451248s)\n"),
        ],
    );
    let open = |node: &str| {
        let config = RenderConfig::at(44_100);
        plane(&sva_engine::render(&g, node, config, &Tier::default()).expect("a render"))
    };
    assert_eq!(open("click"), [1.0, 1.0]);
    let bend = open("bend");
    assert_eq!(bend.len(), 2);
    assert!(bend[1] > 0.02, "{bend:?}");
}
