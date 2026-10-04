// Concern: proves a symbolic derivative agrees with the slope of the closed form it came from | Non-concern: differentiating an atom (sva-formula) | IO: (a Render) -> a derivative

use crate::fixtures::graph_of;
use sva_engine::{Ask, Output, RenderConfig, Representation, Source, Tier, answer, render};
use sva_samples::{Extent, PSYCHOACOUSTIC_V1, Rows};

/// The first component of a spectral sum's rows over `extent`.
fn sampled(sum: &sva_formula::SpectralSum, rate: u32, extent: Extent) -> Vec<f64> {
    let rows = Rows::of_spectral_sum_or_point(
        sum,
        None,
        (sva_samples::Grid::of(rate), &PSYCHOACOUSTIC_V1),
        &sva_formula::Opaque,
    )
    .expect("the sum has rows");
    let mut planes = rows.planes(extent.start, extent.end).expect("samples");
    planes.swap_remove(0)
}

/// A four-times-oversampled five-point difference of the closed form itself is the reference, its
/// own truncation error four orders below the tolerance being claimed.
#[test]
fn a_symbolic_derivative_agrees_with_a_four_times_finite_difference() {
    let g = graph_of(
        "derivative",
        &[("tone", "sin(2*pi*100*t) + 0.5*sin(2*pi*250*t)\n")],
    );
    let config = RenderConfig::seconds(44_100, 0.05).asking(vec![Ask {
        node: "tone".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "tone", config, &Tier::default()).expect("a law");
    let id = held.id("tone").expect("the root");

    let found = answer(&held, id, Representation::Derivative).expect("a derivative");
    assert_eq!(found.source, Source::Exact);
    assert_eq!(found.rate, None);
    let Output::Symbolic(slope) = found.value else {
        panic!("a law's derivative is symbolic");
    };

    let rate = 4 * 44_100;
    let horizon = Extent::secs(rate, 0.0, 0.05);
    let differentiated = sampled(&slope, rate, horizon);
    let law = sampled(
        &sva_engine::spectral_sum_of(&held.tys, id, sva_engine::Var::T).expect("the law"),
        rate,
        horizon,
    );

    let step = f64::from(rate);
    let mut worst = 0.0f64;
    for i in 2..law.len() - 2 {
        let five_point =
            (-law[i + 2] + 8.0 * law[i + 1] - 8.0 * law[i - 1] + law[i - 2]) * step / 12.0;
        worst = worst.max((five_point - differentiated[i]).abs() / 1_600.0);
    }
    assert!(worst < 1e-6, "symbolic against 4R difference: {worst}");
}

/// An envelope is the root of a squared atom sum, so it answers as a closed form and never as a
/// block follower while the node is still one.
#[test]
fn a_laws_envelope_is_symbolic_and_positive() {
    let g = graph_of("envelope", &[("tone", "sin(2*pi*100*t)\n")]);
    let config = RenderConfig::seconds(44_100, 0.05).asking(vec![Ask {
        node: "tone".to_string(),
        representation: Representation::Envelope { frame_secs: None },
    }]);
    let held = render(&g, "tone", config, &Tier::default()).expect("a law");
    let id = held.id("tone").expect("the root");
    let found = answer(&held, id, Representation::Envelope { frame_secs: None }).expect("one");
    let Output::Symbolic(squared) = found.value else {
        panic!("a law's envelope is symbolic");
    };
    let buffer = sampled(&squared, 44_100, Extent::secs(44_100, 0.0, 0.05));
    for i in (0..buffer.len()).step_by(97) {
        assert!(
            (buffer[i] - 1.0).abs() < 1e-9,
            "a unit sine's squared envelope is one: {}",
            buffer[i]
        );
    }
}
