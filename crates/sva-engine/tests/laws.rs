// Concern: proves a closed form answers off its own spectral sum with no buffer behind it | Non-concern: what a collapse produces (sva-samples) | IO: (a composition) -> Answer

mod fixtures;

use fixtures::graph_of;
use sva_engine::{
    Ask, Cache, MemoryCache, Output, RenderConfig, Representation, Source, answer, render,
};

const CHORD: &str = "sin(2*pi*256*t) + sin(2*pi*512*t) + sin(2*pi*768*t)\n";

#[test]
fn lines_answers_without_collapse() {
    let g = graph_of("lines", &[("chord", CHORD)]);
    let config = RenderConfig::seconds(44_100, 1.0).asking(vec![Ask {
        node: "chord".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "chord", config, None).expect("a law renders nothing");
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
    let held = render(&g, "chord", config, None).expect("a law");
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
    let held = render(&g, "chord", RenderConfig::seconds(8_192, 1.0), None).expect("audio");
    let root = held.id("chord").expect("the root");
    let buffer = held.buffer(root).expect("the root is materialized");
    assert_eq!(buffer.len(), 8_192);
    assert_eq!(held.labels[&root].source, Source::Exact);
}

/// A warm render is the same bytes as a cold one: a hit is served whole, not re-collapsed.
#[test]
fn a_warm_collapse_is_the_cold_one_byte_for_byte() {
    let g = graph_of("warm", &[("chord", CHORD)]);
    let store = MemoryCache::new();
    let config = || RenderConfig::seconds(8_192, 0.5);
    let cold = render(&g, "chord", config(), Some(&store)).expect("a cold render");
    let cold_root = cold.id("chord").expect("the root");
    let cold_samples = cold.buffer(cold_root).expect("a buffer").clone();
    assert!(store.held_bytes() == 0 || store.max_bytes() > 0);

    let warm = render(&g, "chord", config(), Some(&store)).expect("a warm render");
    let warm_root = warm.id("chord").expect("the root");
    assert_eq!(warm.buffer(warm_root).expect("a buffer"), &cold_samples);
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
    let held = render(&g, "band", RenderConfig::seconds(44_100, 1.0), None)
        .expect("a series through a filter");
    let root = held.id("band").expect("the root");
    assert!(held.tys.ty(root).has_dual());

    let sum = held.symbolic.get(&root).expect("the filtered series");
    let [lane] = sum.lanes.as_slice() else {
        panic!("one lane");
    };
    let [series] = lane.series.as_slice() else {
        panic!("one series, not {} of them", lane.series.len());
    };
    let shaped = sva_formula::lines(series, 20_000.0, -200.0, 0.0);
    let flat = sva_formula::lines(&sva_formula::noise(11, 1.0, 0.0), 20_000.0, -200.0, 0.0);
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

/// FORMAT 14.2: a reading in both tables runs against whichever the node holds, and a pair
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
        let held = render(&g, "tone", config, None).expect("a tone");
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

/// FORMAT 14.1: `spectrum` of a dual is the exact line list, the same content as `lines`
/// in spectrum shape. Sixteen of a node's twenty thousand lines is not that content. A peak
/// budget picks peaks out of an estimate, and a pair's spectrum is not an estimate.
#[test]
fn spectrum_of_a_pair_is_its_full_line_list_or_labeled_measured() {
    let g = graph_of("full-spectrum", &[("chord", CHORD)]);
    let config = RenderConfig::seconds(44_100, 1.0).asking(vec![Ask {
        node: "chord".to_string(),
        representation: Representation::Lines,
    }]);
    let held = render(&g, "chord", config, None).expect("a law");
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

/// A pole times an indicator leaves A, so no closed form carries the analytic signal. The
/// reading is still an answer: one collapse, measured off the grid, never a refusal. A line
/// spectrum, whose analytic signal stays in A, keeps its exact envelope.
#[test]
fn envelope_of_a_damped_stack_is_measured_not_refused() {
    let damped = "crop(exp(-3*t)*sin(2*pi*440*t), 0s, 0.05s)\n";
    let reading = |name: &str, body: &str| {
        let g = graph_of(name, &[("node", body)]);
        let config = RenderConfig::seconds(48_000, 0.1).asking(vec![Ask {
            node: "node".to_string(),
            representation: Representation::Envelope { frame_secs: None },
        }]);
        let held = render(&g, "node", config, None).expect("a law renders");
        let root = held.id("node").expect("the root");
        answer(&held, root, Representation::Envelope { frame_secs: None })
            .unwrap_or_else(|e| panic!("{name}: {e}"))
    };

    let measured = reading("envelope-damped", damped);
    assert_eq!(measured.source, Source::Measured);
    assert_eq!(
        measured.rate,
        Some(48_000),
        "a measured reading names its rate"
    );
    let Output::Envelope(frames) = measured.value else {
        panic!("an envelope answers frames off the grid");
    };
    assert!(!frames.is_empty(), "the window carries frames");
    let inside = frames.first().expect("the first frame").rms;
    let after = frames.last().expect("the last frame").rms;
    assert!(inside > 0.1, "the strike sounds: {inside}");
    assert!(after < inside / 100.0, "the crop ends it: {after}");

    // A Gaussian times a sine leaves A for a different reason, and takes the same route.
    let gaussian = reading("envelope-gaussian", "exp(-100*t*t)*sin(2*pi*440*t)\n");
    assert_eq!(gaussian.source, Source::Measured);

    let exact = reading("envelope-lines", "sin(2*pi*440*t)\n");
    assert_eq!(exact.source, Source::Exact, "a line spectrum stays exact");
    assert_eq!(exact.rate, None, "an exact envelope needs no rate");
}

/// A reciprocal of a closed form in t composes no atom sum at all, so nothing symbolic answers
/// and no collapse off a spectral sum does either. Row 4 over the written closed form still
/// reaches the grid, and the envelope of that buffer is the reading.
#[test]
fn envelope_of_a_rational_in_t_is_measured() {
    let g = graph_of("envelope-rational", &[("node", "1/(t*t + 1)\n")]);
    let config = RenderConfig::seconds(8_000, 1.0).asking(vec![Ask {
        node: "node".to_string(),
        representation: Representation::Envelope { frame_secs: None },
    }]);
    let held = render(&g, "node", config, None).expect("a rational in t renders");
    let root = held.id("node").expect("the root");
    let taken = answer(&held, root, Representation::Envelope { frame_secs: None })
        .expect("an envelope off the grid, not a refusal");
    assert_eq!(taken.source, Source::Measured);
    assert_eq!(taken.rate, Some(8_000), "a measured reading names its rate");
    let Output::Envelope(frames) = taken.value else {
        panic!("an envelope answers frames off the grid");
    };
    for frame in &frames {
        let want = 1.0 / (frame.t_secs * frame.t_secs + 1.0);
        assert!(
            (frame.peak - want).abs() < 1e-3,
            "at {}s the peak is the closed form's own value: {} against {want}",
            frame.t_secs,
            frame.peak
        );
    }
    let last = frames.last().expect("the window carries frames");
    assert!(last.rms < 0.55, "the tail falls away: {}", last.rms);
}
