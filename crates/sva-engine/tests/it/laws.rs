// Concern: proves a closed form answers off its own spectral sum with no buffer behind it | Non-concern: what a collapse produces (sva-samples) | IO: (a composition) -> Answer

use crate::fixtures::graph_of;
use sva_engine::{Ask, Output, Range, RenderConfig, Representation, Source, Tier, answer, render};

const CHORD: &str = "sin(2*pi*256*t) + sin(2*pi*512*t) + sin(2*pi*768*t)\n";

#[test]
fn lines_answers_without_collapse() {
    let g = graph_of("lines", &[("chord", CHORD)]);
    let config = RenderConfig::seconds(44_100, 1.0).asking(vec![Ask {
        node: "chord".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "chord", config, &Tier::default()).expect("a law renders nothing");
    assert!(
        held.buffers.is_empty(),
        "a line list allocates no buffer: {:?}",
        held.buffers.keys().collect::<Vec<_>>()
    );

    let root = held.id("chord").expect("the root");
    let found = answer(&held, root, Representation::Lines).expect("lines");
    assert_eq!(found.source, Source::Exact);
    assert_eq!(found.rate, None, "a law answers without a rate");
    assert_eq!(found.profile, "psychoacoustic-v1");
    let Output::Lines(lines) = found.value else {
        panic!("expected a line list");
    };
    let mut hz: Vec<f64> = lines.iter().map(|l| l.hz.abs()).collect();
    hz.sort_by(f64::total_cmp);
    hz.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    assert_eq!(hz, vec![256.0, 512.0, 768.0]);
}

#[test]
fn spectrum_of_a_pair_is_exact() {
    let g = graph_of("spectrum", &[("chord", CHORD)]);
    let config = RenderConfig::seconds(44_100, 1.0).asking(vec![Ask {
        node: "chord".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "chord", config, &Tier::default()).expect("a law");
    let root = held.id("chord").expect("the root");
    let found = answer(
        &held,
        root,
        Representation::Spectrum {
            max_peaks: 8,
            frame_secs: None,
        },
    )
    .expect("a spectrum");
    assert_eq!(
        found.source,
        Source::Exact,
        "a pair's spectrum is the line list, not a Welch estimate"
    );
    let Output::Lines(lines) = found.value else {
        panic!("expected the exact line list in spectrum shape");
    };
    assert_eq!(lines.len(), 6, "three lines, each a conjugate pair");
}

/// A render with no reading at all is audio out, and the root collapses by definition.
#[test]
fn the_render_root_collapses_when_the_caller_asks_for_audio() {
    let g = graph_of("audio", &[("chord", CHORD)]);
    let held = render(
        &g,
        "chord",
        RenderConfig::seconds(8_192, 1.0),
        &Tier::default(),
    )
    .expect("audio");
    let root = held.id("chord").expect("the root");
    let buffer = held.output(root).expect("the root is materialized");
    assert_eq!(buffer.len(), 8_192);
    assert_eq!(held.labels[&root].source, Source::Exact);
}

/// A warm render is the same bytes as a cold one: a hit is served whole, not re-collapsed.
#[test]
fn a_warm_collapse_is_the_cold_one_byte_for_byte() {
    let g = graph_of("warm", &[("chord", CHORD)]);
    let store = Tier::default();
    let config = || RenderConfig::seconds(8_192, 0.5);
    let cold = render(&g, "chord", config(), &store).expect("a cold render");
    let cold_root = cold.id("chord").expect("the root");
    let cold_samples = cold.output(cold_root).expect("a buffer").clone();

    let warm = render(&g, "chord", config(), &store).expect("a warm render");
    let warm_root = warm.id("chord").expect("the root");
    assert_eq!(warm.output(warm_root).expect("a buffer"), cold_samples);
}

/// A rational factor folds into a series term by term, so filtered noise is still a pair
/// and every line carries the response read at that line's own frequency.
#[test]
fn noise_through_a_bandpass_is_a_pair_with_shaped_lines() {
    let g = graph_of(
        "shaped",
        &[
            ("flat", "noise(11, period=1, color=0)\n"),
            ("band", "bandpass(@flat, cutoff=1200, q=2)\n"),
        ],
    );
    let held = render(
        &g,
        "band",
        RenderConfig::seconds(44_100, 1.0),
        &Tier::default(),
    )
    .expect("a series through a filter");
    let root = held.id("band").expect("the root");
    assert!(held.tys.ty(root).has_dual());

    let sum = sva_engine::spectral_sum_of(&held.tys, root, sva_engine::Var::T)
        .expect("the filtered series");
    let [lane] = sum.lanes.as_slice() else {
        panic!("one lane");
    };
    let [series] = lane.series.as_slice() else {
        panic!("one series, not {} of them", lane.series.len());
    };
    let shaped = sva_formula::lines(series, 20_000.0, -200.0, 0.0).expect("shaped lines");
    let flat = sva_formula::lines(&sva_formula::noise(11, 1.0, 0.0), 20_000.0, -200.0, 0.0)
        .expect("flat lines");
    assert!(shaped.taken.len() > 100, "{} lines", shaped.taken.len());

    for (a, b) in shaped.taken.iter().zip(&flat.taken) {
        assert!((a.hz - b.hz).abs() < 1e-9, "{} against {}", a.hz, b.hz);
        if b.amp.abs() == 0.0 || a.amp.abs() == 0.0 {
            continue;
        }
        let want = bandpass_db(a.hz, 1200.0, 2.0);
        let found = 20.0 * (a.amp.abs() / b.amp.abs()).log10();
        assert!(
            (found - want).abs() < 0.5,
            "{} Hz carries {found} dB, not {want} dB",
            a.hz
        );
    }
}

/// `H(s) = (w0/q)*s/(s^2 + (w0/q)*s + w0^2)` on `s = 2*pi*i*f`, as FORMAT 10.1 writes it.
fn bandpass_db(hz: f64, cutoff: f64, q: f64) -> f64 {
    let (w, w0) = (std::f64::consts::TAU * hz, std::f64::consts::TAU * cutoff);
    let numerator = w0 / q * w;
    let (re, im) = (w0 * w0 - w * w, w0 / q * w);
    20.0 * (numerator.abs() / (re * re + im * im).sqrt()).log10()
}

/// A reading in both representations runs against whichever the node holds, and a pair
/// holds every partial exactly. An estimator reading 110 Hz as 109.17 is answering a
/// question about a buffer that this node never had to become.
#[test]
fn pitch_of_a_pure_sine_is_exact() {
    let asked = |name: &str, body: &str| {
        let g = graph_of(name, &[("tone", body)]);
        let reading = Representation::Pitch {
            max_notes: 4,
            frame_secs: 0.05,
        };
        let config = RenderConfig::seconds(48_000, 1.0).asking(vec![Ask {
            node: "tone".to_string(),
            representation: reading,
        }]);
        let held = render(&g, "tone", config, &Tier::default()).expect("a tone");
        let root = held.id("tone").expect("the root");
        answer(&held, root, reading).expect("a pitch reading")
    };

    let found = asked("pure-law", "0.5*sin(2*pi*110*t)\n");
    assert_eq!(found.source, Source::Exact);
    assert_eq!(found.rate, None, "a law answers without a rate");
    let Output::Pitch(frames) = &found.value else {
        panic!("expected pitch frames");
    };
    let [frame] = frames.as_slice() else {
        panic!("a line spectrum states one pitch, not one per frame: {frames:?}");
    };
    let [note] = frame.notes.as_slice() else {
        panic!("one sine, one note: {:?}", frame.notes);
    };
    assert_eq!(note.name, "A2");
    assert!((note.hz - 110.0).abs() < 1e-9, "{}", note.hz);
    assert!(note.cents.abs() < 1e-9, "{} cents", note.cents);

    let measured = asked("pure-samples", "sample(0.5*sin(2*pi*110*t))\n");
    assert_eq!(
        measured.source,
        Source::Measured,
        "samples are measured, whatever law was behind them"
    );
    assert!(
        (note.db - 20.0 * 0.5f64.log10()).abs() < 1e-9,
        "one reading, one level scale: a wave of amplitude 0.5 is -6.02 dB on either \
         route, not the half each of its conjugate lines carries: {}",
        note.db
    );
}

/// The exact path names its lines as the measured one names peaks: a fundamental a quarter
/// the amplitude of its octave still heads the series, so the octave is not read as the pitch.
#[test]
fn an_exact_pitch_names_a_weak_fundamental_as_the_head_of_its_series() {
    let body = "0.25*sin(2*pi*220*t) + sin(2*pi*440*t) + 0.5*sin(2*pi*660*t)\n";
    let g = graph_of("weak-fundamental", &[("tone", body)]);
    let reading = Representation::Pitch {
        max_notes: 4,
        frame_secs: 0.05,
    };
    let config = RenderConfig::seconds(48_000, 1.0).asking(vec![Ask {
        node: "tone".to_string(),
        representation: reading,
    }]);
    let held = render(&g, "tone", config, &Tier::default()).expect("a tone");
    let root = held.id("tone").expect("the root");
    let found = answer(&held, root, reading).expect("an exact pitch");
    let Output::Pitch(frames) = &found.value else {
        panic!("expected pitch frames");
    };
    let [frame] = frames.as_slice() else {
        panic!("one frame: {frames:?}");
    };
    let named: Vec<(&str, Option<f64>)> = frame
        .notes
        .iter()
        .map(|n| (n.name.as_str(), n.harmonic_of))
        .collect();
    assert_eq!(
        named,
        [("A4", Some(220.0)), ("E5", Some(220.0)), ("A3", None)],
        "loudest first, both partials 220 Hz's"
    );
}

/// A series' line lands beside the lane's own at the same frequency, unmerged. Its amplitude
/// is their complex sum: `sin` less the series' first term at 440 Hz is silence, not +6 dB.
#[test]
fn pitch_sums_the_complex_amplitudes_of_lines_at_one_frequency() {
    let body = "sin(2*pi*440*t) - sum(k, 1, 2, sin(2*pi*440*k*t)/k)\n";
    let g = graph_of("cancelled", &[("tone", body)]);
    let asked = |reading: Representation| {
        let config = RenderConfig::seconds(48_000, 1.0).asking(vec![Ask {
            node: "tone".to_string(),
            representation: reading,
        }]);
        let held = render(&g, "tone", config, &Tier::default()).expect("a tone");
        let root = held.id("tone").expect("the root");
        answer(&held, root, reading)
            .expect("an exact reading")
            .value
    };

    let Output::Lines(lines) = asked(Representation::Lines) else {
        panic!("expected lines");
    };
    let at_440 = lines.iter().filter(|l| l.hz == 440.0).count();
    assert_eq!(at_440, 2, "the series' line stands unmerged: {lines:?}");

    let Output::Pitch(frames) = asked(Representation::Pitch {
        max_notes: 4,
        frame_secs: 0.05,
    }) else {
        panic!("expected pitch frames");
    };
    let [frame] = frames.as_slice() else {
        panic!("one frame: {frames:?}");
    };
    let [note] = frame.notes.as_slice() else {
        panic!("440 Hz cancels, 880 Hz is left: {:?}", frame.notes);
    };
    assert_eq!(note.hz, 880.0);
    assert!(
        (note.db - 20.0 * 0.5f64.log10()).abs() < 1e-12,
        "{} dB",
        note.db
    );
}

/// An exact pitch frame starts where the render reads its root from: the stated start, else
/// the one its support gives, 0 for an unwindowed line. A render never asked for `pitch`
/// decided no start, and refuses rather than inventing one.
#[test]
fn an_exact_pitch_frame_starts_where_the_render_reads_from() {
    let g = graph_of("pitch-start", &[("tone", "sin(2*pi*440*t)\n")]);
    let pitch = Representation::Pitch {
        max_notes: 4,
        frame_secs: 0.05,
    };
    let read = |asked: Representation, start: Option<i64>| {
        let mut config = RenderConfig::at(48_000).asking(vec![Ask {
            node: "tone".to_string(),
            representation: asked,
        }]);
        config.range = Range { start, end: None };
        let held = render(&g, "tone", config, &Tier::default()).expect("a tone");
        let root = held.id("tone").expect("the root");
        answer(&held, root, pitch)
    };
    let starts = |start: Option<i64>| {
        let found = read(pitch, start).expect("an exact pitch");
        assert_eq!(found.source, Source::Exact);
        let Output::Pitch(frames) = found.value else {
            panic!("expected pitch frames");
        };
        frames.iter().map(|f| f.t_secs).collect::<Vec<f64>>()
    };
    assert_eq!(starts(Some(24_000)), vec![0.5]);
    assert_eq!(starts(None), vec![0.0]);

    let unasked = read(Representation::Lines, Some(24_000)).expect_err("no start decided");
    assert_eq!(unasked.code(), "engine.not_materialized");
}

/// `spectrum` of a dual is the exact line list, the same content as `lines`
/// in spectrum shape. Sixteen of a node's twenty thousand lines is not that content. A peak
/// budget picks peaks out of an estimate, and a pair's spectrum is not an estimate.
#[test]
fn spectrum_of_a_pair_is_its_full_line_list_or_labeled_measured() {
    let g = graph_of("full-spectrum", &[("chord", CHORD)]);
    let config = RenderConfig::seconds(44_100, 1.0).asking(vec![Ask {
        node: "chord".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "chord", config, &Tier::default()).expect("a law");
    let root = held.id("chord").expect("the root");

    let Output::Lines(listed) = answer(&held, root, Representation::Lines)
        .expect("a line list")
        .value
    else {
        panic!("expected lines");
    };

    let spectrum = |max_peaks| {
        answer(
            &held,
            root,
            Representation::Spectrum {
                max_peaks,
                frame_secs: None,
            },
        )
        .expect("a spectrum")
    };

    let whole = spectrum(listed.len());
    assert_eq!(whole.source, Source::Exact);
    let Output::Lines(found) = &whole.value else {
        panic!("expected lines");
    };
    assert_eq!(
        found.len(),
        listed.len(),
        "the same content as `lines`, in spectrum shape"
    );

    let budgeted = spectrum(1);
    assert_eq!(budgeted.source, Source::Exact);
    let Output::Lines(found) = &budgeted.value else {
        panic!("expected lines");
    };
    assert_eq!(
        found.len(),
        listed.len(),
        "a budget that would cut the list is not what `spectrum` of a pair answers"
    );
}

/// A pole times an indicator leaves A, so no closed form carries the analytic signal, and a
/// reciprocal in t composes no atom sum at all. Neither has an exact envelope, and neither is
/// measured unasked: each refuses with why and names `sample(...)`, which answers measured.
#[test]
fn envelope_without_an_exact_answer_refuses_and_names_the_measured_one() {
    let reading = |name: &str, body: &str| {
        let g = graph_of(name, &[("node", body)]);
        let config = RenderConfig::seconds(48_000, 0.1).asking(vec![Ask {
            node: "node".to_string(),
            representation: Representation::Envelope { frame_secs: None },
        }]);
        let held = render(&g, "node", config, &Tier::default()).expect("a law renders");
        let root = held.id("node").expect("the root");
        answer(&held, root, Representation::Envelope { frame_secs: None })
    };

    for (body, code) in [
        (
            "crop(exp(-3*t)*sin(2*pi*440*t), 0s, 0.05s)",
            "cast.left_algebra",
        ),
        ("exp(-100*t*t)*sin(2*pi*440*t)", "cast.left_algebra"),
        ("1/(t*t + 1)", "cast.left_algebra"),
    ] {
        let refused = reading(body, &format!("{body}\n"))
            .expect_err("no exact envelope, and none measured unasked");
        assert_eq!(refused.code(), code, "{body}");
        let said = refused.to_string();
        assert!(said.contains("no exact envelope"), "{body}: {said}");
        assert!(said.contains("sample("), "{body}: {said}");

        let measured = reading(body, &format!("sample({body})\n"))
            .unwrap_or_else(|e| panic!("{body}: the measured envelope asked for: {e}"));
        assert_eq!(measured.source, Source::Measured, "{body}");
        assert_eq!(measured.rate, Some(48_000), "{body}");
    }

    let exact = reading("lines", "sin(2*pi*440*t)\n").expect("an exact envelope");
    assert_eq!(exact.source, Source::Exact, "a line spectrum stays exact");
    assert_eq!(exact.rate, None, "an exact envelope needs no rate");
}

/// A decay steep enough that its weight from t = 0 passes the largest double is anchored at
/// its onset, so it reads its own value there rather than stopping the render.
#[test]
fn a_steep_decay_cropped_at_a_late_onset_reads_its_value_there() {
    for (name, decay) in [
        ("exp", "crop(exp(-(t - 3s)*400), 3s, 4s)\n"),
        ("struck", "crop(string(220, at=3s), 0s, 4s)\n"),
    ] {
        let g = graph_of(name, &[("decay", decay)]);
        let held = render(
            &g,
            "decay",
            RenderConfig::seconds(44_100, 4.0),
            &Tier::default(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        let root = held.id("decay").expect("the root");
        let samples = held.output(root).expect("a buffer").plane(0).to_vec();
        assert!(samples[..3 * 44_100].iter().all(|v| *v == 0.0), "{name}");
        assert!(samples.iter().all(|v| v.is_finite()), "{name}");
        assert!(samples[3 * 44_100..].iter().any(|v| *v != 0.0), "{name}");
    }
    let g = graph_of("onset", &[("decay", "crop(exp(-(t - 3s)*400), 3s, 4s)\n")]);
    let held = render(
        &g,
        "decay",
        RenderConfig::seconds(44_100, 4.0),
        &Tier::default(),
    )
    .expect("renders");
    let samples = held
        .output(held.id("decay").expect("the root"))
        .expect("a buffer");
    let at = |n: usize| samples.plane(0)[3 * 44_100 + n];
    assert!((at(0) - 1.0).abs() < 1e-12, "{}", at(0));
    assert!(
        (at(1) - (-400.0f64 / 44_100.0).exp()).abs() < 1e-12,
        "{}",
        at(1)
    );
}
