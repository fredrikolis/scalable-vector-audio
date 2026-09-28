// Concern: proves each unary name reaches its signature, renderer and point sampler, and what step and inf mean | Non-concern: what the others compute (sva-formula, sva-samples) | IO: (a name) -> Buffer

mod fixtures;

use fixtures::graph_of;
use sva_engine::{RenderConfig, render};
use sva_formula::Unary;

fn rendered(name: &str, body: &str) -> Vec<f64> {
    let g = graph_of(name, &[("src", "2*sin(2*pi*300*t)\n"), ("node", body)]);
    let held = render(&g, "node", RenderConfig::seconds(8_000, 0.01), None)
        .unwrap_or_else(|e| panic!("{name} in `{body}`: {e}"));
    let root = held.id("node").expect("the root");
    held.output(root)
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

fn at_rate(files: &[(&str, &str)]) -> Result<Vec<f64>, sva_engine::EngineError> {
    let g = graph_of("step", files);
    let held = render(&g, "node", RenderConfig::seconds(8_000, 0.5), None)?;
    let root = held.id("node").expect("the root");
    Ok(held.output(root).expect("a buffer").plane(0).to_vec())
}

/// `step` of a rising line is the crop opening where the line crosses zero, one at zero
/// itself; a window opening at `inf` holds nothing whatever it crops; `inf` arithmetic that
/// names no number, stands in a term that moves or reaches a filter or a named argument,
/// refuses.
#[test]
fn step_is_the_crop_at_its_zero_and_a_window_at_inf_holds_nothing() {
    let tone = "sin(2*pi*300*t)";
    for (step, crop) in [("t - 0.25", "0.25s"), ("3*t - 0.3", "0.1s")] {
        let stepped = at_rate(&[("node", &format!("{tone}*step({step})\n"))]);
        let cropped = at_rate(&[("node", &format!("{tone}*crop(1, {crop}, inf)\n"))]);
        assert_eq!(
            stepped.expect("step"),
            cropped.expect("crop"),
            "step({step})"
        );
    }
    let at_zero = at_rate(&[("node", "step(t - 0.25)\n")]).expect("step");
    assert_eq!((at_zero[1_999], at_zero[2_000]), (0.0, 1.0));

    let gated = "r = inf\ncrop(exp(-(t - r)/0.3), r, 3600s) + crop(1, 0s, r)*exp(-r)\n";
    let silent = at_rate(&[("node", gated)]).expect("a gate at inf");
    assert!(silent.iter().all(|v| v.to_bits() == 0), "{silent:?}");

    for undefined in [
        "sin(t)*(inf - inf)\n",
        "r = inf\ncrop(exp(-(t - r)/0.3), 0s, 1s)\n",
        "crop(lowpass(sample(sin(t)), cutoff=pow(2, inf)), 0s, 1s)\n",
        "crop(sat(sample(sin(t)), drive=pow(2, inf)), 0s, 1s)\n",
    ] {
        let e = at_rate(&[("node", undefined)]).expect_err(undefined);
        assert_eq!(e.code(), "engine.infinite_value", "{undefined}: {e}");
    }
}
