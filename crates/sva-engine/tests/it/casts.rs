// Concern: proves each written crossing keeps the value it declares and is keyed on its own | Non-concern: what the value denotes (sva-formula) | IO: (a composition) -> a Buffer or a refusal

use crate::fixtures::graph_of;
use sva_engine::{Cast, Codomain, Held, RenderConfig, Source, Tier, Ty, Var, render};

const CHORD: &str = "sin(2*pi*256*t) + sin(2*pi*512*t)\n";

fn samples(name: &str, files: &[(&str, &str)], root: &str, rate: u32, secs: f64) -> Vec<f64> {
    let g = graph_of(name, files);
    let held = render(
        &g,
        root,
        RenderConfig::seconds(rate, secs),
        &Tier::default(),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    let id = held.id(root).expect("the root");
    held.output(id).expect("a rendered root").plane(0).to_vec()
}

/// `fourier` then `ifourier` selects one axis and then the other: no value changes.
#[test]
fn fourier_then_ifourier_of_a_chord_is_the_same_normal() {
    let plain = samples("plain", &[("chord", CHORD)], "chord", 8_192, 1.0);
    let turned = samples(
        "turned",
        &[("chord", CHORD), ("both", "ifourier(fourier(@chord))\n")],
        "both",
        8_192,
        1.0,
    );
    assert_eq!(plain.len(), turned.len());
    for (i, (a, b)) in plain.iter().zip(&turned).enumerate() {
        assert!((a - b).abs() < 1e-9, "sample {i}: {a} against {b}");
    }
}

/// Exact from sample zero, not only in the steady-state overlap.
#[test]
fn stft_and_istft_round_trip_from_sample_zero() {
    let plain = samples("direct", &[("chord", CHORD)], "chord", 44_100, 0.25);
    let round = samples(
        "roundtrip",
        &[
            ("chord", CHORD),
            (
                "back",
                "istft(stft(sample(crop(@chord, 0s, 0.25s)), window=1024sp, hop=256sp))\n",
            ),
        ],
        "back",
        44_100,
        0.25,
    );
    assert_eq!(plain.len(), round.len());
    for (i, (a, b)) in plain.iter().zip(&round).enumerate() {
        assert!((a - b).abs() < 1e-12, "sample {i}: {a} against {b}");
    }
}

/// The label says exact, which an edited round trip could not.
#[test]
fn an_unedited_round_trip_is_labelled_exact() {
    let g = graph_of(
        "labelled",
        &[
            ("chord", CHORD),
            (
                "back",
                "istft(stft(sample(crop(@chord, 0s, 0.25s)), window=1024sp, hop=256sp))\n",
            ),
        ],
    );
    let held = render(
        &g,
        "back",
        RenderConfig::seconds(8_192, 0.25),
        &Tier::default(),
    )
    .expect("a round trip");
    let root = held.id("back").expect("the root");
    assert_eq!(held.labels[&root].source, Source::Exact);
}

/// Two crossings of one closed form are two values, so neither serves the other's key.
#[test]
fn each_cast_has_its_own_key() {
    let g = graph_of(
        "keys",
        &[
            ("chord", CHORD),
            ("as_law", "@chord*1\n"),
            ("as_spectrum", "fourier(@chord)\n"),
        ],
    );
    let key = |root: &str| {
        let typing = sva_engine::types(&g, root).expect("a typing");
        sva_engine::identity(&typing, typing.id(root).expect("the root")).expect("an identity")
    };
    let (law_key, turned_key) = (key("as_law"), key("as_spectrum"));
    assert_ne!(law_key, turned_key, "a crossing is a key of its own");
}

/// A retyping cast holds a closed form, so a spectrum written beside one composes into the product
/// around it: FORMAT 7.3's three steps, written by hand, place the line at the response.
#[test]
fn a_spectrum_product_under_ifourier_composes() {
    let g = graph_of(
        "spectrum-product",
        &[
            ("shell", "sin(2*pi*185*t)\n"),
            ("tilt", "exp(0 - pow(f/2400, 2))\n"),
            ("voiced", "ifourier(fourier(@shell) * @tilt)\n"),
        ],
    );
    let held = render(
        &g,
        "voiced",
        RenderConfig::seconds(44_100, 0.1),
        &Tier::default(),
    )
    .expect("a cast inside a product composes");
    let id = held.id("voiced").expect("the root");
    assert!(held.tys.ty(id).has_dual());
    let buffer = held.output(id).expect("a collapsed pair");
    let want = (-(185.0f64 / 2400.0).powi(2)).exp();
    let peak = (0..buffer.len())
        .map(|i| buffer.at(0, i).abs())
        .fold(0.0f64, f64::max);
    assert!(
        (peak - want).abs() < 1e-6,
        "the line carries {peak}, not the response {want}"
    );
}

/// FORMAT 3.3: the cast states which axis the expression above it is written on.
#[test]
fn fourier_types_form_t_to_form_f() {
    let written = Ty::form(Var::T, true, Codomain::Real);
    let crossed = Cast::Fourier.resolve(&[written]).expect("a dual crosses");
    assert_eq!((crossed.held, crossed.dual), (Held::Form(Var::F), true));
    let back = Cast::IFourier
        .resolve(&[crossed])
        .expect("and crosses back");
    assert_eq!((back.held, back.dual), (Held::Form(Var::T), true));
}

/// A form in `f` is its dual on the grid, so one the Fourier dual rules cannot dual refuses on every asker.
#[test]
fn a_form_in_f_with_no_dual_refuses_on_every_asker() {
    let g = graph_of(
        "undualed-spectrum",
        &[
            ("bank", "sum(k, 1, 8, exp(0 - pow((f - 100*k)/10, 2)))\n"),
            ("master", "@bank\n"),
        ],
    );
    let refused = render(
        &g,
        "master",
        RenderConfig::seconds(8_000, 0.1),
        &Tier::default(),
    )
    .err()
    .expect("a form in f with no dual");
    assert_eq!(refused.code(), "cast.left_algebra", "{refused}");

    let config = sva_engine::StreamConfig {
        block: 100,
        channels: None,
        render: RenderConfig::seconds(8_000, 0.1),
    };
    let target = sva_ast::parse_expr("@master").expect("a ref");
    let opened = crate::fixtures::Now::now(sva_engine::Stream::open(
        &g,
        &target,
        config,
        &Tier::default(),
    ));
    let refused = opened.err().expect("a stream refuses it too");
    assert_eq!(refused.code(), "cast.left_algebra", "{refused}");
}
