// Concern: proves one unary name reaches its signature, its renderer and the point sampler | Non-concern: what any of them computes (sva-formula, sva-samples) | IO: (a name) -> two Buffers

mod fixtures;

use fixtures::graph_of;
use sva_engine::{RenderConfig, render};
use sva_formula::Unary;

fn rendered(name: &str, body: &str) -> Vec<f64> {
    let g = graph_of(name, &[("src", "2*sin(2*pi*300*t)\n"), ("node", body)]);
    let held = render(&g, "node", RenderConfig::seconds(8_000, 0.01), None)
        .unwrap_or_else(|e| panic!("{name} in `{body}`: {e}"));
    let root = held.id("node").expect("the root");
    held.buffer(root)
        .unwrap_or_else(|| panic!("{name}: a buffer"))
        .plane(0)
        .to_vec()
}

/// A name a site does not know refuses rather than answering.
#[test]
fn every_unary_operator_reaches_the_renderer_and_the_point_sampler_by_its_one_name() {
    for op in Unary::ALL {
        let name = op.name();
        assert!(
            sva_engine::overload::signature(name).is_some(),
            "{name} has a signature"
        );

        let shaded = rendered(name, &format!("{name}(sample(0.5 + 0*t))\n"));
        assert!(
            shaded.iter().all(|s| s.is_finite()),
            "{name} over samples: {shaded:?}"
        );
        assert_eq!(shaded.len(), 80, "{name} over samples");

        let pointed = rendered(
            name,
            &format!("{name}(1 + abs(lowpass(@src, cutoff=800, q=0.7)))\n"),
        );
        assert!(
            pointed.iter().all(|s| s.is_finite()),
            "{name} over a filtered operand: {pointed:?}"
        );
        assert!(
            pointed.iter().any(|s| *s != 0.0),
            "{name} over a filtered operand sounded"
        );
    }
}
