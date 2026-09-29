// Concern: proves what reading another node yields, per the representation it holds | Non-concern: what a node evaluates to | IO: (a composition) -> Read or a refusal

use crate::fixtures::graph_of;
use sva_engine::instantiate::{instantiate, resolve_ref_path};
use sva_engine::schedule_from;
use sva_engine::{Ask, EngineError, Held, RenderConfig, Representation, Var, render, types};
use sva_engine::{PSYCHOACOUSTIC_V1, Read, resolve, symbolic_hash};

#[test]
fn dependencies_precede_dependents() {
    let g = graph_of(
        "order",
        &[
            ("kick", "sin(2*pi*50*t)\n"),
            ("lead", "@kick*0.5 + @kick(t - 0.01s)*0.3\n"),
        ],
    );
    let instances = instantiate(&g, "lead", PSYCHOACOUSTIC_V1).expect("a graph that instantiates");
    let order = schedule_from(&instances, &["lead".to_string()])
        .expect("a schedule")
        .groups
        .concat();
    let kick_pos = order.iter().position(|p| p == "kick").expect("kick");
    let lead_pos = order.iter().position(|p| p == "lead").expect("lead");
    assert!(kick_pos < lead_pos);
    assert_eq!(
        order.iter().filter(|p| p.as_str() == "kick").count(),
        1,
        "two refs from one node are one member of the walk"
    );
}

#[test]
fn a_walk_above_the_root_is_a_refusal_the_engine_can_locate() {
    assert_eq!(
        resolve_ref_path("piano/bench", "../../escape").expect_err("above the root"),
        EngineError::RefAboveRoot("piano/bench".to_string(), "../../escape".to_string()),
    );
}

/// A shift is a node of its own, so two reads a fraction of a sample apart are two
/// expressions and neither can be served out of the other's key.
#[test]
fn a_fractional_shift_hashes_as_a_new_expression() {
    let hash = |name: &str, body: &str| {
        let g = graph_of(name, &[("src", "sin(2*pi*220*t)\n"), ("node", body)]);
        let typing = types(&g, "node").expect("a law");
        let id = typing.id("node").expect("the root");
        symbolic_hash(&typing, id, sva_engine::Var::T).expect("a law hashes")
    };
    let plain = hash("plain", "@src\n");
    let moved = hash("moved", "@src(t - 0.000001s)\n");
    let barely = hash("barely", "@src(t - 0.0000011s)\n");
    assert_ne!(plain, moved, "a shift is a new expression");
    assert_ne!(
        moved, barely,
        "two shifts under one sample apart are two expressions"
    );
}

/// An `sp` offset on a sampled ref moves the reading by whole lattice samples.
#[test]
fn an_sp_offset_read_is_an_integer_index() {
    let rendered = |name: &str, body: &str| {
        let g = graph_of(
            name,
            &[("grid", "chaigne_askenfelt(261.63)\n"), ("node", body)],
        );
        let held = render(&g, "node", RenderConfig::seconds(44_100, 0.01), None)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let id = held.id("node").expect("the root");
        held.output(id).expect("a rendered read").plane(0).to_vec()
    };
    let plain = rendered("plain-grid", "@grid*0.5\n");
    let moved = rendered("moved-grid", "@grid(t - 2sp)*0.5\n");
    assert_eq!(moved[0], 0.0, "before the read is founded");
    assert_eq!(moved[1], 0.0);
    for i in 2..plain.len() {
        assert!(
            (moved[i] - plain[i - 2]).abs() < 1e-12,
            "sample {i}: {} against {}",
            moved[i],
            plain[i - 2]
        );
    }
}

/// A duration on a sampled ref is an index offset wherever it is a whole sample count at
/// the observation's rate, and a bar resolved against a tempo is such a duration.
#[test]
fn a_bar_offset_that_lands_on_the_grid_reads_a_sampled_ref() {
    let mut g = graph_of(
        "bar-offset",
        &[
            ("grid", "sample(sin(2*pi*220*t))\n"),
            ("node", "@grid(t - 1b)*0.5\n"),
        ],
    );
    g.resolve_bar_spans(sva_ast::PerBar {
        seconds: 2.0,
        per: 1.0,
    });
    let held = render(&g, "node", RenderConfig::seconds(8_000, 2.5), None)
        .expect("a bar offset that lands on the grid");
    let id = held.id("node").expect("the root");
    let buffer = held.output(id).expect("a rendered read");
    let delay = 16_000;
    for i in 0..buffer.len() {
        let at = i as f64 - f64::from(delay);
        let want = 0.5 * (std::f64::consts::TAU * 220.0 * at / 8_000.0).sin();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// A closed form ref is inlined into the reading closed form, and nothing is held for it.
#[test]
fn a_law_ref_substitutes_and_allocates_no_buffer() {
    let g = graph_of(
        "substituted",
        &[
            ("src", "sin(2*pi*220*t)\n"),
            ("node", "@src*0.5 + @src(t - 0.01s)\n"),
        ],
    );
    let config = RenderConfig::seconds(8_192, 0.25).asking(vec![Ask {
        node: "node".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "node", config, None).expect("a law");
    assert!(held.buffers.is_empty(), "no buffer stands behind a law");

    let id = held.id("node").expect("the root");
    let Read::Substitute(form) =
        resolve(&held.tys, id, Held::Form(Var::T)).expect("a law substitutes")
    else {
        panic!("a law ref substitutes rather than hitting a buffer");
    };
    assert!(
        sva_engine::nodes_in(&form.body).is_empty(),
        "the referenced law is inlined, not left as a node"
    );
}

/// `sp` is one step of the lattice in seconds, so a closed form read `2sp` back is that closed
/// form moved, exact at any rate.
#[test]
fn a_law_read_at_a_grid_offset_renders() {
    let g = graph_of(
        "indexed-law",
        &[
            ("src", "sin(2*pi*220*t)\n"),
            ("node", "@src(t - 2sp)*0.5\n"),
        ],
    );
    let held = render(&g, "node", RenderConfig::seconds(8_000, 0.01), None)
        .expect("a law read on the grid");
    let id = held.id("node").expect("the root");
    let buffer = held.output(id).expect("a rendered read");
    assert_eq!(buffer.len(), 80);
    for i in 0..buffer.len() {
        let at = i as f64 / 8_000.0 - 2.0 / 44_100.0;
        let want = 0.5 * (std::f64::consts::TAU * 220.0 * at).sin();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// A ref read at a time no offset names is the callee's closed form at that time: the read tiles,
/// and the reading node is a point-sampled closed form rather than a pair.
#[test]
fn a_law_ref_tiled_by_modulo_point_samples() {
    let g = graph_of(
        "tiled",
        &[("src", "sin(2*pi*100*t)\n"), ("node", "@src(t % 0.0037)\n")],
    );
    let typing = types(&g, "node").expect("a tiled read types");
    let held = typing.id("node").expect("the root");
    let warped = typing.ty(held);
    assert_eq!(
        (warped.held, warped.dual),
        (Held::Form(Var::T), false),
        "a warped time leaves A"
    );

    let rendered =
        render(&g, "node", RenderConfig::seconds(8_000, 0.05), None).expect("a tiled read");
    let root = rendered.id("node").expect("the root");
    let buffer = rendered.output(root).expect("a point-sampled law");
    for i in 0..buffer.len() {
        let want = (std::f64::consts::TAU * 100.0 * ((i as f64 / 8_000.0) % 0.0037)).sin();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// A ref naming one number is that number, so a template may write its pitches as offsets
/// from a key node rather than as literals.
#[test]
fn a_key_times_a_semitone_offset_folds() {
    let g = graph_of(
        "key-offset",
        &[
            ("variables/key", "D3\n"),
            ("voice", "sin(2*pi*f0*t)\n"),
            ("node", "@voice(t, f0=@variables/key*3st)\n"),
        ],
    );
    let config = RenderConfig::seconds(8_192, 0.25).asking(vec![Ask {
        node: "node".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "node", config, None).expect("a folded key");
    let id = held.id("node").expect("the root");
    let found = sva_engine::answer(&held, id, Representation::Lines).expect("lines");
    let sva_engine::Output::Lines(lines) = found.value else {
        panic!("expected a line list");
    };
    let hz: Vec<f64> = lines
        .iter()
        .map(|l| l.hz.abs())
        .filter(|h| *h > 0.0)
        .collect();
    let want = sva_formula::note::frequency("D3").expect("D3") * 2f64.powf(3.0 / 12.0);
    assert!(
        hz.iter().all(|h| (h - want).abs() < 1e-9),
        "a minor third above D3 is {want} Hz, got {hz:?}"
    );
}

/// Folding a ref is for the ones naming one number. A series and a delta each hold a closed form,
/// and reading either under arithmetic composes it rather than flattening it.
#[test]
fn a_ref_that_is_not_one_number_composes_as_a_law() {
    let g = graph_of(
        "not-a-number",
        &[
            ("saw", "sum(k, 1, 10, sin(2*pi*k*220*t)/k)\n"),
            ("imp", "delta(t)\n"),
            ("voiced", "@saw*2\n"),
            ("struck", "@imp*2\n"),
        ],
    );

    let held = render(&g, "voiced", RenderConfig::seconds(44_100, 0.05), None)
        .expect("a series read under a product");
    let buffer = held
        .output(held.id("voiced").expect("the root"))
        .expect("a rendered series");
    let peak = (0..buffer.len()).fold(0.0f64, |m, i| m.max(buffer.at(0, i).abs()));
    assert!(peak > 1.0, "a series ref keeps its partials, peak {peak}");

    let config = RenderConfig::seconds(44_100, 0.05).asking(vec![Ask {
        node: "struck".to_string(),
        representation: Representation::Atoms,
    }]);
    let held = render(&g, "struck", config, None).expect("a delta read under a product");
    let id = held.id("struck").expect("the root");
    let sva_engine::Output::Atoms(atoms) = sva_engine::answer(&held, id, Representation::Atoms)
        .expect("atoms")
        .value
    else {
        panic!("expected an atom list");
    };
    assert!(
        atoms.iter().any(|a| a.contains("delta")),
        "a delta ref stays a delta: {atoms:?}"
    );
}

/// A time written one per component is a per-lane substitution: each lane of the callee is
/// read at the time its own lane names.
#[test]
fn a_per_channel_shift_on_a_law_substitutes_per_lane() {
    let g = graph_of(
        "per-lane",
        &[
            ("wide", "join(sin(2*pi*220*t), sin(2*pi*330*t))\n"),
            ("taps", "@wide(join(t - 0.01s, t - 0.02s))\n"),
        ],
    );
    let held = render(&g, "taps", RenderConfig::seconds(44_100, 0.05), None)
        .expect("a law read once per component");
    let buffer = held
        .output(held.id("taps").expect("the root"))
        .expect("two lanes");
    assert_eq!(buffer.width, 2);
    for i in [0usize, 441, 1000] {
        let t = i as f64 / 44_100.0;
        let left = (std::f64::consts::TAU * 220.0 * (t - 0.01)).sin();
        let right = (std::f64::consts::TAU * 330.0 * (t - 0.02)).sin();
        assert!(
            (buffer.at(0, i) - left).abs() < 1e-12,
            "left at {i}: {} against {left}",
            buffer.at(0, i)
        );
        assert!(
            (buffer.at(1, i) - right).abs() < 1e-12,
            "right at {i}: {} against {right}",
            buffer.at(1, i)
        );
    }
}

/// A buffer read carries one offset, so two times at once is two reads, and the refusal
/// writes both of them out.
#[test]
fn a_per_channel_shift_on_samples_names_each_read() {
    let g = graph_of(
        "per-lane-samples",
        &[
            ("wide", "sample(join(sin(2*pi*220*t), sin(2*pi*330*t)))\n"),
            ("taps", "@wide(join(t - 0.01s, t - 0.02s))\n"),
        ],
    );
    let e = types(&g, "taps").expect_err("samples read at two times refuse");
    let EngineError::Refused(d) = &e else {
        panic!("expected a written refusal, got {e:?}");
    };
    assert_eq!(d.code, "engine.per_lane_read_on_samples");
    assert!(d.message.contains("t - 0.01"), "{}", d.message);
    assert!(d.message.contains("t - 0.02"), "{}", d.message);
    assert!(d.help.contains("ch(@wide("), "{}", d.help);
}

/// A mono buffer has no second component to read, so the repair names one read per time.
#[test]
fn a_per_channel_shift_on_a_mono_buffer_names_one_read_per_time() {
    let g = graph_of(
        "per-lane-mono",
        &[
            ("mono", "sample(sin(2*pi*220*t))\n"),
            ("taps", "@mono(join(t - 0.01s, t - 0.02s))\n"),
        ],
    );
    let e = types(&g, "taps").expect_err("one buffer, two times");
    let EngineError::Refused(d) = &e else {
        panic!("expected a written refusal, got {e:?}");
    };
    assert_eq!(d.code, "engine.per_lane_read_on_samples");
    assert!(
        d.help.contains("join(@mono(t - 0.01), @mono(t - 0.02))"),
        "{}",
        d.help
    );
    assert!(
        !d.help.contains("ch("),
        "a mono buffer has no component 1: {}",
        d.help
    );
}

/// FORMAT 15.2: a ref inlines symbolically, and a join of pairs is a pair per lane. An
/// exact reading composes through every construct the closed form is written with, not only the
/// sum and the product.
#[test]
fn lines_compose_through_a_join_of_crops() {
    let g = graph_of(
        "compose-through",
        &[
            ("tone", "sin(2*pi*440*t)\n"),
            ("other", "sin(2*pi*550*t)\n"),
            ("wide", "join(@tone(t), @other(t))\n"),
            (
                "windows",
                "join(crop(@tone(t), 0s, 1s), crop(@other(t), 0s, 1s))\n",
            ),
            ("shouldered", "crop(@tone(t), 0s, 1s, rise=0.1, fall=0.1)\n"),
            ("one", "ch(@wide(t), 1)\n"),
            ("squared", "pow(@tone(t), 2)\n"),
            ("stacked", "sum(k, 1, 3, @tone(t)/k)\n"),
        ],
    );
    let read = |node: &str, representation| {
        let config = RenderConfig::seconds(8_192, 1.0).asking(vec![Ask {
            node: node.to_string(),
            representation,
        }]);
        let held = render(&g, node, config, None).unwrap_or_else(|e| panic!("{node}: {e}"));
        let id = held.id(node).unwrap_or_else(|| panic!("{node} typed"));
        sva_engine::answer(&held, id, representation).unwrap_or_else(|e| panic!("{node}: {e}"))
    };
    let hz = |node: &str| {
        let sva_engine::Output::Lines(lines) = read(node, Representation::Lines).value else {
            panic!("{node}: expected lines");
        };
        let mut out: Vec<i64> = lines.iter().map(|l| l.hz.round() as i64).collect();
        out.sort_unstable();
        out
    };
    assert_eq!(hz("wide"), vec![-550, -440, 440, 550], "a join of two refs");
    assert_eq!(hz("one"), vec![-550, 550], "one component of a joined pair");

    let count = |node: &str| {
        let sva_engine::Output::Atoms(atoms) = read(node, Representation::Atoms).value else {
            panic!("{node}: expected atoms");
        };
        atoms
    };
    let windows = count("windows");
    assert_eq!(windows.len(), 4, "two cropped lines per lane: {windows:?}");
    assert!(
        windows.iter().all(|a| a.contains("indicator")),
        "each carries its window: {windows:?}"
    );
    let shouldered = count("shouldered");
    assert_eq!(
        shouldered.len(),
        14,
        "seven window pieces times two line atoms: {shouldered:?}"
    );
    assert!(
        shouldered
            .iter()
            .all(|a| a.contains("exponential") && a.contains("indicator")),
        "each atom is the tone under its own piece of the window: {shouldered:?}"
    );
    assert_eq!(
        hz("squared"),
        vec![-880, 0, 880],
        "a ref under pow is the law it inlines to, the constant term its line at zero"
    );
    assert_eq!(
        hz("stacked"),
        vec![-440, -440, -440, 440, 440, 440],
        "a ref under a finite sum is one term per index"
    );
}

/// FORMAT 6.5 and 12: a modal bank is a finite sum of damped sinusoids, so it multiplies,
/// shifts, crops and reads like any other pair rather than only rendering bare.
#[test]
fn a_modal_bank_times_an_envelope_lists_its_modes() {
    let g = graph_of(
        "modal-composes",
        &[
            ("pluck", "string(D3, modes=6)\n"),
            ("voiced", "@pluck(t)*exp(0 - 3*t)\n"),
            ("echoed", "@pluck(t) + 0.8*@pluck(t - 15ms)\n"),
            ("windowed", "crop(@pluck(t), 0s, 1s)\n"),
        ],
    );
    let atoms = |node: &str| {
        let representation = Representation::Atoms;
        let config = RenderConfig::seconds(8_192, 1.0).asking(vec![Ask {
            node: node.to_string(),
            representation,
        }]);
        let held = render(&g, node, config, None).unwrap_or_else(|e| panic!("{node}: {e}"));
        let id = held.id(node).unwrap_or_else(|| panic!("{node} typed"));
        let sva_engine::Output::Atoms(atoms) = sva_engine::answer(&held, id, representation)
            .unwrap_or_else(|e| panic!("{node}: {e}"))
            .value
        else {
            panic!("{node}: expected atoms");
        };
        atoms
    };
    let voiced = atoms("voiced");
    assert_eq!(
        voiced.len(),
        12,
        "six modes, one pole pair each: {voiced:?}"
    );
    assert!(
        voiced
            .iter()
            .all(|a| a.contains("exponential") && a.contains("indicator")),
        "each mode rings from the strike under its own decay: {voiced:?}"
    );
    assert_eq!(atoms("echoed").len(), 24, "the bank and its shifted copy");
    assert_eq!(
        atoms("windowed").len(),
        12,
        "a crop of a bank is a bank cropped"
    );
    assert_eq!(
        atoms("pluck").len(),
        12,
        "the bare bank lists its own modes"
    );
}

/// The first `n` samples of `body` at 100 Hz, where `form` is the ramp `t` as a closed form
/// and `held` the same ramp as samples.
fn first(name: &str, body: &str, n: usize) -> Result<Vec<f64>, EngineError> {
    let root = format!("{body}\n");
    let g = graph_of(
        name,
        &[("form", "t\n"), ("held", "sample(t)\n"), ("root", &root)],
    );
    let held = render(
        &g,
        "root",
        RenderConfig::seconds(100, n as f64 / 100.0),
        None,
    )?;
    let root = held.id("root").expect("the root");
    Ok(held.output(root).expect("samples").plane(0).to_vec())
}

fn near(got: &[f64], want: &[f64]) -> bool {
    got.len() == want.len() && got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-12)
}

/// A reflected read keeps its sign on either representation: `0.11s - t` counts down.
#[test]
fn a_reflected_read_counts_down_on_a_form_and_on_samples() {
    for (name, body) in [
        ("form-rev", "@form(0.11s - t)"),
        ("held-rev", "@held(0.11s - t)"),
        ("held-neg", "@held(-1*(t - 0.11s))"),
        ("held-sp", "@held(4851sp - t)"),
    ] {
        let got = first(name, body, 3).expect("a reflected read renders");
        assert!(near(&got, &[0.11, 0.10, 0.09]), "{body}: {got:?}");
    }
}

/// Samples read at any multiple of `t` land where that multiple says: on lattice samples
/// where it is whole there, between them otherwise, read there by the kernel.
#[test]
fn a_scaled_read_of_samples_lands_on_or_between_samples() {
    let got = first("held-2t", "@held(2*t - 0.02s)", 3).expect("a whole scale renders");
    assert!(near(&got, &[-0.02, 0.0, 0.02]), "{got:?}");
    let got = first("held-half", "@held(0.5*t)", 3).expect("half a sample renders");
    let want = [0.0, 0.005, 0.01];
    assert!(
        got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-9),
        "{got:?}"
    );
}

/// Seconds and lattice steps add in one offset.
#[test]
fn an_offset_in_seconds_and_steps_reads_their_sum() {
    let got = first("mixed", "@held(t - 0.02s - 1sp)", 3).expect("a mixed offset renders");
    let step = 1.0 / 44_100.0;
    assert!(near(&got, &[-0.02 - step, -0.01 - step, -step]), "{got:?}");
}
