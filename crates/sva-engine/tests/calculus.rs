// Concern: proves a symbolic derivative agrees with the slope of the closed form it came from | Non-concern: differentiating an atom (sva-formula) | IO: (a Render) -> a derivative

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Ask, Output, RenderConfig, Representation, Source, answer, render};
use sva_samples::{AliasScore, Horizon, PSYCHOACOUSTIC_V1, of_spectral_sum};

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
    let held = render(&g, "tone", config, None).expect("a law");
    let id = held.id("tone").expect("the root");

    let found = answer(&held, id, Representation::Derivative).expect("a derivative");
    assert_eq!(found.source, Source::Exact);
    assert_eq!(found.rate, None);
    let Output::Symbolic(slope) = found.value else {
        panic!("a law's derivative is symbolic");
    };

    let rate = 4 * 44_100;
    let horizon = Horizon::secs(0.0, 0.05);
    let (differentiated, _) = of_spectral_sum(
        &slope,
        rate,
        horizon,
        &PSYCHOACOUSTIC_V1,
        AliasScore::NotAsked,
    )
    .expect("the slope collapses");
    let (law, _) = of_spectral_sum(
        &sva_engine::spectral_sum_of(&held.tys, id, sva_engine::Var::T).expect("the law"),
        rate,
        horizon,
        &PSYCHOACOUSTIC_V1,
        AliasScore::NotAsked,
    )
    .expect("the law collapses");

    let step = f64::from(rate);
    let mut worst = 0.0f64;
    for i in 2..law.len() - 2 {
        let five_point = (-law.at(0, i + 2) + 8.0 * law.at(0, i + 1) - 8.0 * law.at(0, i - 1)
            + law.at(0, i - 2))
            * step
            / 12.0;
        worst = worst.max((five_point - differentiated.at(0, i)).abs() / 1_600.0);
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
    let held = render(&g, "tone", config, None).expect("a law");
    let id = held.id("tone").expect("the root");
    let found = answer(&held, id, Representation::Envelope { frame_secs: None }).expect("one");
    let Output::Symbolic(squared) = found.value else {
        panic!("a law's envelope is symbolic");
    };
    let (buffer, _) = of_spectral_sum(
        &squared,
        44_100,
        Horizon::secs(0.0, 0.05),
        &PSYCHOACOUSTIC_V1,
        AliasScore::NotAsked,
    )
    .expect("the squared envelope collapses");
    for i in (0..buffer.len()).step_by(97) {
        assert!(
            (buffer.at(0, i) - 1.0).abs() < 1e-9,
            "a unit sine's squared envelope is one: {}",
            buffer.at(0, i)
        );
    }
}
