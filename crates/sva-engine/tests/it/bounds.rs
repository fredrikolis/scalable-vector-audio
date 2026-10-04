// Concern: proves each cut a magnitude bound makes zeroes only samples the unpruned node holds under the prune level | Non-concern: where a cut lands (faded.rs) | IO: (a composition) -> cuts, samples

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{PSYCHOACOUSTIC_V1, Profile, Render, RenderConfig, Tier};

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

fn rendered(g: &Graph, target: &str, prune_db: f64) -> Render {
    let config = RenderConfig {
        profile: Profile {
            prune_db,
            ..PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 4.0)
    };
    sva_engine::render(g, target, config, &Tier::default())
        .unwrap_or_else(|e| panic!("{target}: {e}"))
}

fn plane(r: &Render) -> Vec<f64> {
    r.output(r.root).expect("the root").plane(0).to_vec()
}

/// No profile level is under zero: nothing is cut.
fn unpruned(g: &Graph, node: &str) -> Vec<f64> {
    plane(&rendered(g, &format!("s_{node}"), f64::NEG_INFINITY))
}

/// Every cut any render makes holds: the node it zeroes, rendered with nothing cut, stays
/// under the level from its cut on.
#[test]
fn every_cut_zeroes_only_samples_under_the_prune_level() {
    let g = graph();
    let level = PSYCHOACOUSTIC_V1.prune_level();
    let (mut checked, mut wrong) = (0, Vec::new());
    for (name, _) in NODES {
        let r = rendered(&g, &format!("s_{name}"), PSYCHOACOUSTIC_V1.prune_db);
        let pruned = r.labels[&r.root].pruned.clone().expect("a stated level");
        for (node, from) in pruned.cuts {
            let truth = unpruned(&g, &node);
            let from = usize::try_from(from.max(0)).expect("a sample");
            let peak = truth
                .get(from..)
                .unwrap_or_default()
                .iter()
                .fold(0.0f64, |m, v| m.max(v.abs()));
            if peak >= level {
                wrong.push(format!(
                    "{node} cut from {from} under {name}, reaching {peak:e}"
                ));
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
        plane(&rendered(&g, "s_q", PSYCHOACOUSTIC_V1.prune_db)),
        plane(&rendered(&g, "s_q_inline", PSYCHOACOUSTIC_V1.prune_db)),
    );
    let peak = |x: &[f64]| x.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    assert!(peak(&inline) > 9e8, "{}", peak(&inline));
    assert_eq!(refs.len(), inline.len());
    for (n, (a, b)) in refs.iter().zip(&inline).enumerate() {
        assert!((a - b).abs() <= 1e-9 * b.abs(), "sample {n}: {a} {b}");
    }
}
