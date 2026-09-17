// Concern: proves a form no atom sum reaches still meets the grid, one instant at a time | Non-concern: which row a form that does reach one takes (sva-samples) | IO: (a composition) -> a Buffer

mod fixtures;

use std::f64::consts::TAU;

use fixtures::graph_of;
use sva_engine::{Ask, Detail, RenderConfig, Representation, Rule, Source, render};

/// The operand crossed `ifourier`, so there is no spectral sum and no written closed form either.
#[test]
fn a_nonlinearity_over_a_filtered_pair_point_samples() {
    let g = graph_of(
        "nonlinear-filter",
        &[
            ("src", "2*sin(2*pi*300*t)\n"),
            ("body", "sat(lowpass(@src, cutoff=800, q=0.7))\n"),
        ],
    );
    let held = render(&g, "body", RenderConfig::seconds(44_100, 0.05), None)
        .expect("a point-sampled nonlinearity");
    let root = held.id("body").expect("the root");
    let buffer = held.buffer(root).expect("a rendered law");
    assert_eq!(buffer.len(), 2_205);
    let peak = buffer
        .plane(0)
        .iter()
        .fold(0f64, |held, s| held.max(s.abs()));
    assert!(peak > 0.5, "a saturated 300 Hz tone sounded: {peak}");
    assert!(peak <= 1.0, "sat clamps at one: {peak}");
}

/// FORMAT 9.1 row 4: point sampling is measured, against the alias the grid lost, for a reading
/// that asks for the score.
#[test]
fn the_point_sampled_label_carries_alias_db() {
    let g = graph_of(
        "labelled",
        &[
            ("src", "2*sin(2*pi*3000*t)\n"),
            ("body", "sat(lowpass(@src, cutoff=8000, q=0.7))\n"),
        ],
    );
    let asked = RenderConfig::seconds(44_100, 0.1).asking(vec![Ask {
        node: "body".to_string(),
        representation: Representation::Alias { oversample: 4 },
    }]);
    let held = render(&g, "body", asked, None).expect("a reading");
    let root = held.id("body").expect("the root");
    let label = held.labels.get(&root).expect("a label beside the buffer");
    assert_eq!(label.source, Source::Measured);
    assert_eq!(label.rule(), Rule::PointSampled);
    assert_eq!(label.rate, 44_100);
    let Detail::Point { alias_db, .. } = label.detail else {
        panic!(
            "a point sampling carries an alias score, not {:?}",
            label.detail
        )
    };
    let alias_db = alias_db.expect("the reading asked for the score");
    assert!(
        alias_db.is_finite(),
        "an alias score was measured: {alias_db}"
    );
    assert!(
        alias_db < 0.0,
        "the alias sits under the signal: {alias_db}"
    );
}

/// The shape every identity kick is built from.
#[test]
fn the_identity_burst_shape_renders() {
    let g = graph_of(
        "burst",
        &[
            ("strike", "exp(-40*t)\n"),
            (
                "burst",
                "sat(bandpass(bandpass(rand(3), cutoff=180, q=1.4), cutoff=180, q=1.4), \
                 drive=2)*@strike\n",
            ),
        ],
    );
    let held = render(&g, "burst", RenderConfig::seconds(44_100, 0.05), None)
        .expect("the burst shape renders");
    let root = held.id("burst").expect("the root");
    let buffer = held.buffer(root).expect("a rendered burst");
    assert_eq!(buffer.len(), 2_205);
    assert!(
        buffer.plane(0).iter().all(|s| s.is_finite()),
        "every sample is a number"
    );
}

/// `join` and `ch` name components, so a point sampling reads the one each names.
#[test]
fn a_joined_pair_point_samples_each_component_it_names() {
    let g = graph_of(
        "components",
        &[
            ("left", "sin(2*pi*300*t)\n"),
            ("right", "0.25 + 0*t\n"),
            (
                "wide",
                "join(sat(lowpass(@left, cutoff=800, q=0.7)), @right)\n",
            ),
            ("picked", "ch(@wide, 1)\n"),
        ],
    );
    let held = render(&g, "wide", RenderConfig::seconds(8_000, 0.01), None).expect("a join");
    let wide = held
        .buffer(held.id("wide").expect("the root"))
        .expect("both");
    assert_eq!(wide.width, 2);
    assert!(
        wide.plane(1).iter().all(|s| (s - 0.25).abs() < 1e-12),
        "the right component is the constant it was written as"
    );
    assert!(
        wide.plane(0).iter().any(|s| s.abs() > 0.1),
        "the left component is the saturated tone"
    );

    let held = render(&g, "picked", RenderConfig::seconds(8_000, 0.01), None).expect("a channel");
    let picked = held
        .buffer(held.id("picked").expect("the root"))
        .expect("one component");
    assert_eq!(picked.width, 1);
    assert!(
        picked.plane(0).iter().all(|s| (s - 0.25).abs() < 1e-12),
        "ch(x, 1) is the second component, not the first"
    );
}

/// A series has a value at an instant once it is truncated, and FORMAT 6.2 truncates it
/// once: the terms its own coefficient keeps above the profile's floor.
#[test]
fn a_neumann_series_under_a_product_renders() {
    let g = graph_of(
        "neumann",
        &[
            ("src", "sin(2*pi*220*t)\n"),
            ("echo", "@src + 0.5*self(t - 0.01s)\n"),
            ("node", "@echo(t)*tanh(t)\n"),
        ],
    );
    let held = render(&g, "node", RenderConfig::seconds(44_100, 0.05), None)
        .expect("a series under a product");
    let root = held.id("node").expect("the root");
    let buffer = held.buffer(root).expect("a point-sampled law");
    let taken = 5;
    for i in (0..buffer.len()).step_by(37) {
        let t = i as f64 / 44_100.0;
        let want: f64 = (0..taken)
            .map(|k| 0.5f64.powi(k) * (TAU * 220.0 * (t - 0.01 * f64::from(k))).sin())
            .sum::<f64>()
            * t.tanh();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-7,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// A hash keyed on quantized time holds one value per step. Point sampling reads the hash at
/// each instant, so every sample of a step is the one value that step names.
#[test]
fn sample_and_hold_noise_renders_measured() {
    let g = graph_of(
        "sample-and-hold",
        &[("held", "rand(t - t % 0.0625, seed=17)\n")],
    );
    let held = render(&g, "held", RenderConfig::seconds(44_100, 0.25), None)
        .expect("a keyed hash of a moving key renders");
    let id = held.id("held").expect("the root");
    let buffer = held.buffer(id).expect("a rendered hold");
    let label = held.labels.get(&id).expect("a label");
    assert_eq!(label.source, Source::Measured, "point sampling is measured");
    assert!(
        matches!(
            label.detail,
            Detail::Point {
                rule: Rule::PointSampled,
                ..
            }
        ),
        "{:?}",
        label.detail
    );

    let step = (0.0625 * 44_100.0) as usize;
    let first = buffer.at(0, 0);
    for i in 0..step {
        assert_eq!(
            buffer.at(0, i),
            first,
            "sample {i} is inside the first step"
        );
    }
    let second = buffer.at(0, step + 1);
    assert_ne!(second, first, "the next step draws again");
    for i in step + 1..2 * step {
        assert_eq!(
            buffer.at(0, i),
            second,
            "sample {i} is inside the second step"
        );
    }
    assert!(
        (0.0..=1.0).contains(&first) && (0.0..=1.0).contains(&second),
        "a draw is in the unit interval: {first}, {second}"
    );
}

/// A width-2 closed form with no spectral sum is two lanes, and the pointwise path reads each of them
/// where the join puts it.
#[test]
fn a_panned_ct_law_point_samples_both_lanes() {
    let g = graph_of(
        "panned",
        &[
            ("src", "tanh(3*sin(2*pi*220*t))\n"),
            ("pan", "join(@src*cos(0.3), @src*sin(0.3))\n"),
        ],
    );
    let held = render(&g, "pan", RenderConfig::seconds(44_100, 0.01), None)
        .expect("a joined law point-samples");
    let id = held.id("pan").expect("the root");
    let buffer = held.buffer(id).expect("a rendered pan");
    assert_eq!(buffer.width, 2, "the join names two components");
    for i in [1usize, 17, 123] {
        let mono = (3.0 * (TAU * 220.0 * i as f64 / 44_100.0).sin()).tanh();
        assert!(
            (buffer.at(0, i) - mono * (0.3f64).cos()).abs() < 1e-12,
            "left at {i}: {}",
            buffer.at(0, i)
        );
        assert!(
            (buffer.at(1, i) - mono * (0.3f64).sin()).abs() < 1e-12,
            "right at {i}: {}",
            buffer.at(1, i)
        );
    }
}

/// A detuned unison stack sums series whose phase is not affine, so no term is an atom.
#[test]
fn a_supersaw_stack_of_series_point_samples() {
    let g = graph_of(
        "supersaw",
        &[
            ("open", "0.32*t - 0.03*(1 - exp(-t/0.03))\n"),
            (
                "one",
                "saw(2*pi*220*(1 + off)*t + 2*pi*220*off*@open(t))*0.3\n",
            ),
            (
                "stack",
                "@one(t, off=-0.011) + @one(t, off=0) + @one(t, off=0.011)\n",
            ),
        ],
    );
    let held = render(&g, "stack", RenderConfig::seconds(44_100, 0.02), None)
        .expect("a stack of warped series reaches the grid");
    let root = held.id("stack").expect("the root");
    let buffer = held.buffer(root).expect("a rendered stack");
    assert_eq!(buffer.len(), 882);
    assert!(
        buffer.plane(0).iter().all(|s| s.is_finite()),
        "every sample is a number"
    );
    let peak = buffer
        .plane(0)
        .iter()
        .fold(0f64, |so_far, s| so_far.max(s.abs()));
    assert!(peak > 0.1, "three detuned saws sounded: {peak}");
    let label = held.labels.get(&root).expect("a label beside the buffer");
    assert_eq!(label.source, Source::Measured);
    assert_eq!(label.rule(), Rule::PointSampled);
}
