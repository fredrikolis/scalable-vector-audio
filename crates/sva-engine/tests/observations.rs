// Concern: proves every answer says which reading ran and under which profile | Non-concern: the arithmetic of any one reading (sva-samples) | IO: (a Render) -> Answer

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Ask, Output, Render, RenderConfig, Representation, Source, answer, render};

const CHORD: &str = "sin(2*pi*256*t) + sin(2*pi*512*t)\n";

fn rendered(name: &str, body: &str, asks: Vec<&str>) -> Render {
    rendered_of(name, &[("node", body)], asks)
}

fn rendered_of(name: &str, files: &[(&str, &str)], asks: Vec<&str>) -> Render {
    let g = graph_of(name, files);
    let asks = asks
        .into_iter()
        .map(|a| Ask {
            node: "node".to_string(),
            representation: Representation::from_name(a).expect("a known reading"),
        })
        .collect();
    let config = RenderConfig::seconds(8_192, 1.0).asking(asks);
    render(&g, "node", config, None).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn every_representation_reports_source_and_profile() {
    let law = rendered("law-answers", CHORD, vec!["lines"]);
    let id = law.id("node").expect("the root");
    for name in ["lines", "atoms", "derivative", "envelope"] {
        let representation = Representation::from_name(name).expect("a known reading");
        let found = answer(&law, id, representation).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(found.source, Source::Exact, "{name}");
        assert_eq!(found.profile, "psychoacoustic-v1", "{name}");
        assert_eq!(found.rate, None, "{name}: a law answers without a rate");
    }

    let sampled = rendered(
        "sampled-answers",
        "sample(sin(2*pi*256*t))\n",
        vec!["samples"],
    );
    let id = sampled.id("node").expect("the root");
    for name in [
        "samples", "spectrum", "envelope", "bands", "crest", "loudness",
    ] {
        let representation = Representation::from_name(name).expect("a known reading");
        let found = answer(&sampled, id, representation).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(found.source, Source::Measured, "{name}");
        assert_eq!(found.profile, "psychoacoustic-v1", "{name}");
        assert_eq!(found.rate, Some(8_192), "{name}");
    }
}

/// The node's own type decides which `envelope` runs, and `source` says which one did.
#[test]
fn envelope_of_samples_labels_measured() {
    let law = rendered("envelope-law", CHORD, vec!["lines"]);
    let id = law.id("node").expect("the root");
    let symbolic = answer(&law, id, Representation::Envelope { frame_secs: None }).expect("a law");
    assert_eq!(symbolic.source, Source::Exact);
    assert!(matches!(symbolic.value, Output::Symbolic(_)));

    let sampled = rendered(
        "envelope-samples",
        "sample(sin(2*pi*256*t))\n",
        vec!["envelope"],
    );
    let id = sampled.id("node").expect("the root");
    let measured =
        answer(&sampled, id, Representation::Envelope { frame_secs: None }).expect("samples");
    assert_eq!(measured.source, Source::Measured);
    let Output::Envelope(frames) = measured.value else {
        panic!("a block envelope follower answers in frames");
    };
    assert!(!frames.is_empty());
}

/// The one place a reading can be wrong about itself: asking a closed form for samples that were
/// never held refuses instead of inventing them.
#[test]
fn a_reading_off_a_buffer_no_render_held_refuses() {
    let law = rendered("unheld", CHORD, vec!["lines"]);
    let id = law.id("node").expect("the root");
    let refused = answer(&law, id, Representation::Samples).expect_err("nothing was collapsed");
    assert_eq!(refused.code(), "engine.not_materialized");
}

/// A node's parameters are what its own call site bound, and a ledger shares the target's
/// energy out over the buffers this render actually held.
#[test]
fn bindings_and_the_ledger_read_off_the_render_that_ran() {
    let g = graph_of(
        "attributed",
        &[
            ("src", "sin(2*pi*256*t)\n"),
            ("gain", "k = 1\nsample(x*k)\n"),
            ("node", "@gain(t, x=@src, k=0.5)\n"),
        ],
    );
    let config = RenderConfig::seconds(8_192, 0.25).asking(vec![Ask {
        node: "node".to_string(),
        representation: Representation::Samples,
    }]);
    let held = render(&g, "node", config, None).expect("a sampled node");
    let id = held.id("node").expect("the root");

    let instance = held
        .tys
        .paths()
        .find(|(path, _)| path.starts_with("gain("))
        .map(|(_, id)| id)
        .expect("the gain instance");
    let Output::Bindings(bound) = answer(&held, instance, Representation::Bindings)
        .expect("bindings")
        .value
    else {
        panic!("expected the resolved parameters");
    };
    let k = bound.iter().find(|b| b.name == "k").expect("k");
    assert_eq!(k.source, "0.5", "the call site's override, not the default");

    let Output::Ledger(entries) = answer(&held, id, Representation::Ledger { depth: 3 })
        .expect("a ledger")
        .value
    else {
        panic!("expected one entry per node");
    };
    assert!(!entries.is_empty(), "the target is attributed at least");
    assert!(entries.iter().any(|e| e.rms > 0.0), "{entries:?}");
}

/// An alias score is a render against the same closed form oversampled, so it needs a closed form: a node
/// that is samples all the way down says so rather than reporting a missing dual.
#[test]
fn alias_scores_a_collapse_and_refuses_where_no_law_is_behind_it() {
    let collapsed = rendered(
        "aliased",
        "sample(tanh(sin(2*pi*3000*t)*4))\n",
        vec!["alias"],
    );
    let id = collapsed.id("node").expect("the root");
    let found = answer(&collapsed, id, Representation::Alias { oversample: 4 }).expect("a score");
    assert_eq!(found.source, Source::Measured);
    let Output::Alias(score) = found.value else {
        panic!("expected an alias score");
    };
    assert!(score.asr_db.is_finite(), "{score:?}");

    let solved = rendered("no-law", "chaigne_askenfelt(261.63)\n", vec!["alias"]);
    let id = solved.id("node").expect("the root");
    let refused = answer(&solved, id, Representation::Alias { oversample: 4 })
        .expect_err("a solver has no law behind it");
    assert_eq!(refused.code(), "engine.alias_needs_a_closed_form");
}

/// One score stands for the whole node, so it has to be the component that aliases worst: the
/// first alone calls a joined pair clean whenever its quiet half is written first.
#[test]
fn an_alias_score_answers_the_component_that_aliases_worst() {
    const CLEAN: &str = "0.2*sin(2*pi*100*t)";
    const ALIASING: &str = "tanh(6*sin(2*pi*3000*t))";
    let asked = Representation::Alias { oversample: 4 };
    let scored = |name: &str, body: String| {
        let held = rendered(name, &format!("{body}\n"), vec!["alias"]);
        let id = held.id("node").expect("the root");
        let Output::Alias(score) = answer(&held, id, asked).expect("a score").value else {
            panic!("an alias reading answers a score");
        };
        *score
    };

    let alone = scored("alias-loud", ALIASING.to_string());
    let quiet = scored("alias-quiet", CLEAN.to_string());
    assert!(
        alone.nmr_peak_db > quiet.nmr_peak_db,
        "the fixture only says something if one component aliases more: {alone:?} {quiet:?}"
    );

    let quiet_first = scored("alias-quiet-first", format!("join({CLEAN}, {ALIASING})"));
    let loud_first = scored("alias-loud-first", format!("join({ALIASING}, {CLEAN})"));
    assert_eq!(
        quiet_first.asr_db, loud_first.asr_db,
        "the score does not turn on which component was written first"
    );
    assert_eq!(
        quiet_first.asr_db, alone.asr_db,
        "a joined pair scores as its worst half, not as the half written first"
    );
    assert_eq!(quiet_first.audible, alone.audible);
}

/// A peak budget picks peaks out of an estimate; a closed form has none, and a frame length is a
/// question about frames it does not have either.
#[test]
fn a_laws_spectrum_ignores_the_peak_budget_and_refuses_a_frame_length() {
    let law = rendered(
        "budget",
        "sin(2*pi*100*t) + sin(2*pi*200*t) + sin(2*pi*300*t)\n",
        vec!["lines"],
    );
    let id = law.id("node").expect("the root");
    let found = answer(
        &law,
        id,
        Representation::Spectrum {
            max_peaks: 2,
            frame_secs: None,
        },
    )
    .expect("an exact line list");
    assert_eq!(found.source, Source::Exact);
    let Output::Lines(lines) = found.value else {
        panic!("expected lines");
    };
    assert_eq!(
        lines.len(),
        6,
        "three lines, each a conjugate pair, budget or not"
    );

    let refused = answer(
        &law,
        id,
        Representation::Spectrum {
            max_peaks: 2,
            frame_secs: Some(0.05),
        },
    )
    .expect_err("a law has no frames");
    assert_eq!(refused.code(), "engine.observation_needs_samples");
}

/// FORMAT 6.1: a finite `sum` is the type of its term, so the exact reading of a finite sum
/// of sinusoids is every line those terms place, never an empty list labelled exact.
#[test]
fn a_finite_sum_lists_every_line() {
    let law = rendered(
        "finite-sum",
        "sum(k, 2, 6, sin(2*pi*100*k*t))\n",
        vec!["lines"],
    );
    let id = law.id("node").expect("the root");
    let found = answer(&law, id, Representation::Lines).expect("an exact line list");
    assert_eq!(found.source, Source::Exact);
    let Output::Lines(lines) = found.value else {
        panic!("expected lines");
    };
    let mut hz: Vec<i64> = lines
        .iter()
        .filter(|l| l.hz > 0.0)
        .map(|l| l.hz.round() as i64)
        .collect();
    hz.sort_unstable();
    assert_eq!(hz, vec![200, 300, 400, 500, 600]);

    let found = answer(&law, id, Representation::Atoms).expect("an exact atom list");
    let Output::Atoms(atoms) = found.value else {
        panic!("expected atoms");
    };
    assert_eq!(atoms.len(), 10, "one atom per line, both signs");
}

/// FORMAT 14.1: a series answers the terms above the audibility floor with the tail beside
/// them. Three partials round a feedback comb are three waves per echo, and written
/// associativity is not a fact about how many lines a Neumann series has.
#[test]
fn a_long_comb_lists_its_kept_lines_and_names_the_dropped() {
    let law = rendered(
        "long-comb",
        "(sin(2*pi*200*t) + sin(2*pi*400*t) + sin(2*pi*600*t)) + 0.9*self(t - 0.01s)\n",
        vec!["lines"],
    );
    let id = law.id("node").expect("the root");
    let found = answer(&law, id, Representation::Lines).expect("an exact line list");
    assert_eq!(found.source, Source::Exact);
    let Output::Lines(lines) = &found.value else {
        panic!("expected lines");
    };

    let echoes = lines.len() / 6;
    assert!(
        echoes > 1 && lines.len() == echoes * 6,
        "each echo places three partials as six conjugate lines, got {}",
        lines.len()
    );
    let loudest = |set: &[sva_engine::Line]| set.iter().map(|l| l.amp.abs()).fold(0.0f64, f64::max);
    assert!(
        loudest(lines) > 0.0,
        "a truncated series still answers the lines it kept"
    );
    assert!(
        !found.dropped.is_empty(),
        "the terms the floor cut are named, not left to stand in the kept list"
    );
    assert!(
        loudest(&found.dropped) < loudest(lines),
        "what was dropped sits under what was kept"
    );
    assert!(
        found.tail_db.is_some_and(|db| db < 0.0),
        "the tail is stated in dB against the loudest line taken"
    );
}

/// A crop of a series is the series of cropped terms: the window is a pointwise factor, so
/// it lifts off, the terms under it are enumerated, and it goes back on each one — which is
/// also why a cropped term is no line and the line list refuses instead of coming back empty.
#[test]
fn a_cropped_series_lists_its_terms_under_its_window() {
    let bare = rendered(
        "bare-comb",
        "sin(2*pi*200*t) + 0.9*self(t - 0.01s)\n",
        vec!["atoms"],
    );
    let id = bare.id("node").expect("the root");
    let Output::Atoms(loose) = answer(&bare, id, Representation::Atoms)
        .expect("an atom list")
        .value
    else {
        panic!("expected atoms");
    };

    let cropped = rendered_of(
        "cropped-comb",
        &[
            ("comb", "sin(2*pi*200*t) + 0.9*self(t - 0.01s)\n"),
            ("node", "crop(@comb(t), 0s, 0.5s)\n"),
        ],
        vec!["atoms", "lines"],
    );
    let id = cropped.id("node").expect("the root");
    let Output::Atoms(held) = answer(&cropped, id, Representation::Atoms)
        .expect("an atom list")
        .value
    else {
        panic!("expected atoms");
    };
    assert_eq!(
        held.len(),
        loose.len(),
        "a window drops no term it still contains"
    );
    assert!(
        held.iter().all(|a| a.contains("indicator")),
        "every term carries the window: {held:?}"
    );

    let refused = answer(&cropped, id, Representation::Lines)
        .expect_err("a cropped pair places no line, so it lists none");
    assert_eq!(refused.code(), "read.lines_need_unwindowed_lines");
}

/// An envelope gives a line the same width a window does, and only the window used to be
/// named: a Gaussian pulse or a decayed tone listed as `exact, []`, which is the empty list
/// this refusal exists to prevent.
#[test]
fn a_line_an_envelope_widened_is_refused_by_the_factor_that_widened_it() {
    for (name, body, factor) in [
        (
            "gaussian-pulse",
            "exp(-40*t*t)*cos(2*pi*261.6255653005986*t)\n",
            "Gaussian",
        ),
        ("decayed-tone", "exp(-3*t)*sin(2*pi*440*t)\n", "decaying"),
        ("swept-line", "t*sin(2*pi*440*t)\n", "polynomial"),
    ] {
        let held = rendered(name, body, vec!["atoms"]);
        let id = held.id("node").expect("the root");
        for representation in [
            Representation::Lines,
            Representation::Spectrum {
                max_peaks: 8,
                frame_secs: None,
            },
        ] {
            let reading = representation.name();
            let refused = match answer(&held, id, representation) {
                Err(refused) => refused,
                Ok(found) => panic!(
                    "{name}: `{reading}` answered {:?} where it has no line to list",
                    found.value
                ),
            };
            assert_eq!(
                refused.code(),
                "read.lines_need_unwindowed_lines",
                "{name}/{reading}"
            );
            let said = refused.to_string();
            assert!(
                said.contains(factor),
                "{name}/{reading} names what widened it: {said}"
            );
        }
    }
}

/// The transform of a windowed term is its line convolved with the window's own: a shape with
/// a width, which no line holds. An empty list labelled exact says this node has no spectrum,
/// and a list of whatever sits unwindowed beside it says it has a smaller one.
#[test]
fn lines_under_a_window_are_listed_with_their_width_or_refused_by_name() {
    let windowed = rendered(
        "windowed-pair",
        "crop(sin(2*pi*440*t), 0s, 0.25s)\n",
        vec!["atoms"],
    );
    let id = windowed.id("node").expect("the root");
    for representation in [
        Representation::Lines,
        Representation::Spectrum {
            max_peaks: 8,
            frame_secs: None,
        },
        Representation::Pitch {
            max_notes: 4,
            frame_secs: 0.05,
        },
    ] {
        let name = representation.name();
        let refused = match answer(&windowed, id, representation) {
            Err(refused) => refused,
            Ok(held) => panic!(
                "`{name}` answered {:?} where it has no line to list",
                held.value
            ),
        };
        assert_eq!(refused.code(), "read.lines_need_unwindowed_lines", "{name}");
        let said = refused.to_string();
        assert!(said.contains("0.25"), "`{name}` names the window: {said}");
    }

    let mixed = rendered_of(
        "windowed-beside-bare",
        &[
            ("bare", "sin(2*pi*220*t)\n"),
            ("node", "@bare(t) + crop(sin(2*pi*440*t), 0s, 0.25s)\n"),
        ],
        vec!["atoms"],
    );
    let id = mixed.id("node").expect("the root");
    let refused = answer(&mixed, id, Representation::Lines)
        .expect_err("half of a line list reads as the whole of one");
    assert_eq!(refused.code(), "read.lines_need_unwindowed_lines");

    let bare = rendered("bare-pair", "sin(2*pi*440*t)\n", vec!["lines"]);
    let id = bare.id("node").expect("the root");
    let found = answer(&bare, id, Representation::Lines).expect("an exact line list");
    assert_eq!(found.source, Source::Exact);
    let Output::Lines(lines) = found.value else {
        panic!("expected lines");
    };
    assert_eq!(
        lines.len(),
        2,
        "the same term unwindowed is one conjugate pair"
    );
}

/// The same window over a delta train: a delta is at an instant, so a term the window does
/// not contain is dropped outright rather than carried under an indicator.
#[test]
fn a_cropped_delta_train_keeps_only_the_instants_its_window_holds() {
    let counted = |body: &str| {
        let held = rendered("cropped-train", body, vec!["atoms"]);
        let id = held.id("node").expect("the root");
        let Output::Atoms(atoms) = answer(&held, id, Representation::Atoms)
            .expect("an atom list")
            .value
        else {
            panic!("expected atoms");
        };
        atoms.len()
    };

    let loose = counted("sum(k, 0, 9, delta(t - k*0.1s))\n");
    assert_eq!(loose, 10, "ten strikes, ten deltas");
    assert_eq!(
        counted("crop(sum(k, 0, 9, delta(t - k*0.1s)), 0s, 0.25s)\n"),
        3,
        "the half-open window holds the strikes at 0, 0.1 and 0.2 alone"
    );
}

/// `consumes` decides whether the schedule materializes a buffer. Every name the engine
/// resolves is checked, and the four whose verdict turns on the node's own type both ways.
#[test]
fn one_table_says_what_every_named_reading_consumes() {
    use sva_engine::Representation;
    use sva_samples::Consumes;
    let always_closed = ["lines", "atoms", "bindings", "flops"];
    let turns_on_the_type = ["spectrum", "envelope", "derivative", "pitch"];
    let names = [
        "lines",
        "atoms",
        "spectrum",
        "envelope",
        "derivative",
        "samples",
        "ledger",
        "pitch",
        "formants",
        "stereo",
        "bands",
        "crest",
        "loudness",
        "alias",
        "bindings",
        "flops",
    ];
    for name in names {
        let held =
            Representation::from_name(name).unwrap_or_else(|| panic!("`{name}` is a reading"));
        assert_eq!(held.name(), name, "one name, one reading");
        let closed = match always_closed.contains(&name) || turns_on_the_type.contains(&name) {
            true => Consumes::ClosedForm,
            false => Consumes::Buffer,
        };
        assert_eq!(held.consumes(true), closed, "{name} off a closed form");
        let sampled = match always_closed.contains(&name) {
            true => Consumes::ClosedForm,
            false => Consumes::Buffer,
        };
        assert_eq!(held.consumes(false), sampled, "{name} off a sampled node");
    }
}
