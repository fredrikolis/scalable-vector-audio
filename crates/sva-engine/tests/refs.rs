// Concern: proves what reading another node yields, per the representation it holds | Non-concern: what a node evaluates to | IO: (a composition) -> Read or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::instantiate::{instantiate, resolve_ref_path};
use sva_engine::schedule_from;
use sva_engine::{Ask, EngineError, Held, RenderConfig, Representation, Var, render, types};
use sva_engine::{Read, resolve, symbolic_hash};

#[test]
fn dependencies_precede_dependents() {
    let g = graph_of(
        "order",
        &[
            ("kick", "sin(2*pi*50*t)\n"),
            ("lead", "@kick*0.5 + @kick(t - 0.01s)*0.3\n"),
        ],
    );
    let instances = instantiate(&g, "lead").expect("a graph that instantiates");
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

/// An `sp` offset on a sampled ref moves the reading by whole samples, and an offset that
/// names no index refuses rather than being rounded into one.
#[test]
fn an_sp_offset_read_is_an_integer_index() {
    let rendered = |name: &str, body: &str| {
        let g = graph_of(
            name,
            &[("grid", "chaigne_askenfelt(261.63)\n"), ("node", body)],
        );
        let held = render(&g, "node", RenderConfig::seconds(22_050, 0.01), None)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let id = held.id("node").expect("the root");
        held.buffer(id).expect("a rendered read").plane(0).to_vec()
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

    let g = graph_of(
        "fractional",
        &[
            ("grid", "chaigne_askenfelt(261.63)\n"),
            ("node", "@grid(t - 1.5sp)*0.5\n"),
        ],
    );
    assert_eq!(
        types(&g, "node")
            .expect_err("half a sample names no index")
            .code(),
        "ref.fractional_shift_on_samples",
    );
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
    g.resolve_bar_spans(2.0);
    let held = render(&g, "node", RenderConfig::seconds(8_000, 2.5), None)
        .expect("a bar offset that lands on the grid");
    let id = held.id("node").expect("the root");
    let buffer = held.buffer(id).expect("a rendered read");
    let delay = 16_000;
    for i in 0..delay {
        assert_eq!(buffer.at(0, i), 0.0, "sample {i} is before the read");
    }
    for i in delay..buffer.len() {
        let want = 0.5 * (std::f64::consts::TAU * 220.0 * (i - delay) as f64 / 8_000.0).sin();
        assert!(
            (buffer.at(0, i) - want).abs() < 1e-9,
            "sample {i}: {} against {want}",
            buffer.at(0, i)
        );
    }
}

/// An offset a rate turns into half a sample names no index, and the refusal states the
/// count it would have needed rather than rounding into a neighbour.
#[test]
fn a_half_sample_offset_still_refuses_naming_the_count() {
    let g = graph_of(
        "half-sample",
        &[
            ("grid", "sample(sin(2*pi*220*t))\n"),
            ("node", "@grid(t - 0.0025s)*0.5\n"),
        ],
    );
    let Err(refused) = render(&g, "node", RenderConfig::seconds(1_000, 0.05), None) else {
        panic!("2.5 samples names no index");
    };
    assert_eq!(refused.code(), "ref.fractional_shift_on_samples");
    assert!(
        refused.to_string().contains("2.5 samples at 1000 Hz"),
        "the refusal names the count it would need: {refused}"
    );
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
        resolve(&held.tys, id, 0, Held::Form(Var::T)).expect("a law substitutes")
    else {
        panic!("a law ref substitutes rather than hitting a buffer");
    };
    assert!(
        sva_engine::nodes_in(&form.body).is_empty(),
        "the referenced law is inlined, not left as a node"
    );
}

/// `sp` on a read of a closed form puts the whole reading node on the grid, so the closed form it reads is
/// collapsed once and indexed, rather than refusing for having no sampled form.
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
    let buffer = held.buffer(id).expect("a rendered read");
    assert_eq!(buffer.len(), 80);
    for i in 0..2 {
        assert_eq!(buffer.at(0, i), 0.0, "before the read is founded");
    }
    for i in 2..buffer.len() {
        let want = 0.5 * (std::f64::consts::TAU * 220.0 * (i - 2) as f64 / 8_000.0).sin();
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
    let buffer = rendered.buffer(root).expect("a point-sampled law");
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
        .buffer(held.id("voiced").expect("the root"))
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
        .buffer(held.id("taps").expect("the root"))
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
