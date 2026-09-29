// Concern: proves a loop is a continuous series or a discrete loop reading itself by index | Non-concern: running a recurrence (sva-samples) | IO: (a composition) -> Ty, samples or a refusal

use std::f64::consts::TAU;

use crate::fixtures::graph_of;
use sva_engine::{
    Ask, Detail, EngineError, Held, Output, RenderConfig, Representation, Source, Value, When,
    answer, render, types,
};
use sva_formula::Body;

fn form(name: &str, body: &str) -> Result<Held, EngineError> {
    let g = graph_of(name, &[("loop", body)]);
    let typing = types(&g, "loop")?;
    let id = typing.id("loop").expect("the root");
    Ok(typing.ty(id).held)
}

#[test]
fn a_linear_self_keeps_its_dual() {
    let g = graph_of(
        "linear",
        &[("loop", "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n")],
    );
    let typing = types(&g, "loop").expect("a series");
    let id = typing.id("loop").expect("the root");
    assert!(
        typing.ty(id).has_dual(),
        "a comb of resonances is placed analytically, not run on a grid"
    );
    let Value::ClosedForm(form) = typing.value(id) else {
        panic!("a linear loop is a law");
    };
    assert!(
        matches!(form.body, Body::Series(_)),
        "the closed loop is the Neumann series, not the body with self dropped"
    );
}

#[test]
fn an_indexed_self_is_discrete() {
    assert_eq!(
        form(
            "sampled",
            "sample(sin(2*pi*220*t)) + 0.5*self[idx(t) - 1]\n"
        )
        .expect("a recurrence"),
        Held::Sampled,
        "an index read puts the loop on the render's samples"
    );
}

#[test]
fn unit_gain_refuses() {
    let refused = form("unit", "sin(2*pi*220*t) + self(t - 0.01s)\n").expect_err("no value");
    let EngineError::Refused(d) = &refused else {
        panic!("expected a written refusal, got {refused:?}");
    };
    assert_eq!(d.code, "type.self_gain_unbounded");
    assert!(
        d.help.contains("self[idx("),
        "the discrete loop is offered: {}",
        d.help
    );

    let above = form("above", "sin(2*pi*220*t) + 1.5*self(t - 0.01s)\n").expect_err("no value");
    assert_eq!(above.code(), "type.self_gain_unbounded");
}

/// A discrete loop's past is a sequence, so each construct that makes a loop discrete refuses
/// a read of it at an instant, by name, and offers the index read.
#[test]
fn a_discrete_loop_reading_itself_at_an_instant_refuses_at_typing() {
    for (body, construct) in [
        ("sin(2*pi*200*t) + lp(self(t - 17ms), cutoff=2000)\n", "lp"),
        ("sin(2*pi*200*t) + tanh(self(t - 17ms)*2)\n", "tanh"),
        ("sample(sin(2*pi*200*t)) + 0.5*self(t - 17ms)\n", "sample"),
        ("sin(2*pi*200*t) + 0.5*self(t - 1sp)\n", "sp"),
        (
            "sin(2*pi*200*t) + 0.5*self(t - (0.01s + 0.002s*sin(2*pi*3*t)))\n",
            "moves",
        ),
    ] {
        let refused = form("discrete", body).expect_err(body);
        let EngineError::Refused(d) = &refused else {
            panic!("{body}: expected a written refusal, got {refused:?}");
        };
        assert_eq!(d.code, "type.discrete_self_at_time", "{body}");
        assert!(d.message.contains(construct), "{body}: {}", d.message);
        assert!(d.help.contains("self[idx("), "{body}: {}", d.help);
    }
}

const RATES: [u32; 4] = [8_000, 44_100, 48_000, 96_000];

fn at_rate(files: &[(&str, &str)], root: &str, rate: u32, secs: f64) -> sva_engine::Render {
    let g = graph_of("rated", files);
    render(&g, root, RenderConfig::seconds(rate, secs), None)
        .unwrap_or_else(|e| panic!("{root} at {rate}: {e}"))
}

/// Over a formula, `x + 0.5*self(t - 17ms)` is its delay-equation series, exact at every
/// rate whether or not 17 ms is a whole number of its samples.
#[test]
fn a_continuous_loop_renders_its_series_exactly_at_any_rate() {
    let x = |t: f64| match (0.0..0.005).contains(&t) {
        true => (TAU * 200.0 * t).sin(),
        false => 0.0,
    };
    for rate in RATES {
        let held = at_rate(
            &[(
                "loop",
                "crop(sin(2*pi*200*t), 0s, 0.005s) + 0.5*self(t - 17ms)\n",
            )],
            "loop",
            rate,
            0.06,
        );
        let id = held.id("loop").expect("the root");
        for (n, v) in held.output(id).expect("a comb").plane(0).iter().enumerate() {
            let t = n as f64 / f64::from(rate);
            let want: f64 = (0..4)
                .map(|k| 0.5f64.powi(k) * x(t - 0.017 * f64::from(k)))
                .sum();
            assert!(
                (v - want).abs() < 1e-12,
                "{rate}, sample {n}: {v} against {want}"
            );
        }
    }
}

/// The same filter in the loop, read by index, steps on the render's own samples.
#[test]
fn a_filter_over_an_indexed_self_renders_at_any_rate() {
    for rate in RATES {
        let held = at_rate(
            &[(
                "loop",
                "sample(crop(sin(2*pi*200*t), 0s, 0.005s)) + \
                 0.5*lp(self[idx(t - 17ms)], cutoff=2000)\n",
            )],
            "loop",
            rate,
            0.03,
        );
        let id = held.id("loop").expect("the root");
        let plane = held.output(id).expect("a loop").plane(0).to_vec();
        let echo = (0.017 * f64::from(rate)).round() as usize;
        assert!(plane.iter().all(|v| v.is_finite()), "{rate}");
        assert!(
            plane[echo..].iter().any(|v| v.abs() > 1e-3),
            "{rate}: the burst comes back through the filter"
        );
    }
}

/// `self[idx(w)]` with `w` moving reads the loop's nearest past step each sample: alone it has
/// nothing to ring, and a burst comes back once the delay, near 5 ms here, has passed.
#[test]
fn a_loop_reads_its_past_at_a_delay_that_moves() {
    let delay = "self[idx(t - 0.005s - 0.002s*sin(2*pi*0.5*t))]";
    for rate in RATES {
        let alone = format!("lp({delay}, cutoff=2000)\n");
        let held = at_rate(&[("loop", &alone)], "loop", rate, 0.03);
        let id = held.id("loop").expect("the root");
        let plane = held.output(id).expect("a loop").plane(0).to_vec();
        assert!(plane.iter().all(|v| *v == 0.0), "{rate}: nothing rings");
        let rung =
            format!("sample(crop(sin(2*pi*200*t), 0s, 0.002s)) + 0.5*lp({delay}, cutoff=2000)\n");
        let held = at_rate(&[("loop", &rung)], "loop", rate, 0.03);
        let id = held.id("loop").expect("the root");
        let plane = held.output(id).expect("a loop").plane(0).to_vec();
        let at = |secs: f64| (secs * f64::from(rate)) as usize;
        assert!(
            plane[at(0.0021)..at(0.0049)].iter().all(|v| *v == 0.0),
            "{rate}: silent until the delay has passed"
        );
        assert!(
            plane[at(0.005)..at(0.008)].iter().any(|v| v.abs() > 1e-3),
            "{rate}: the burst comes back through the filter"
        );
    }
}

/// Karplus-Strong at 440 Hz: whole samples of delay by index, the fraction by a first-order
/// allpass written out in the loop, `a[n] = c*v[n] + v[n-1] - c*a[n-1]` over the averaged
/// delay line `v`, with `a[n-1] = y[n-1] - x[n-1]`.
#[test]
fn a_karplus_strong_loop_with_an_allpass_fraction_is_its_recurrence() {
    const RATE: u32 = 44_100;
    const N: usize = 99;
    let fraction = f64::from(RATE) / 440.0 - 0.5 - N as f64;
    let c = (1.0 - fraction) / (1.0 + fraction);
    let lp = 0.498;
    let g = graph_of(
        "rated",
        &[
            ("burst", "crop(sample(rand(t, seed=7)), 0s, 0.002s)\n"),
            (
                "string",
                &format!(
                    "@burst + {c}*{lp}*(self[idx(t) - 99] + self[idx(t) - 100]) + \
                     {lp}*(self[idx(t) - 100] + self[idx(t) - 101]) - \
                     {c}*(self[idx(t) - 1] - @burst[idx(t) - 1])\n"
                ),
            ),
        ],
    );
    // A render holds past its pass only the buffers a reading asks for.
    let config = RenderConfig {
        asks: vec![Ask {
            node: "burst".to_string(),
            representation: Representation::Samples,
        }],
        ..RenderConfig::seconds(RATE, 0.05)
    };
    let held = render(&g, "string", config, None).unwrap_or_else(|e| panic!("{e}"));
    let plane = |node: &str| {
        held.output(held.id(node).expect("held"))
            .expect("a buffer")
            .plane(0)
            .to_vec()
    };
    let (x, y) = (plane("burst"), plane("string"));
    let at = |v: &[f64], n: usize, back: usize| n.checked_sub(back).map_or(0.0, |i| v[i]);
    let mut want = vec![0.0; y.len()];
    for n in 0..y.len() {
        want[n] = x[n]
            + c * lp * (at(&want, n, N) + at(&want, n, N + 1))
            + lp * (at(&want, n, N + 1) + at(&want, n, N + 2))
            - c * (at(&want, n, 1) - at(&x, n, 1));
    }
    assert!(
        y[N * 3..].iter().any(|v| v.abs() > 1e-3),
        "the string rings"
    );
    for (n, (v, w)) in y.iter().zip(&want).enumerate() {
        assert!((v - w).abs() < 1e-9, "sample {n}: {v} against {w}");
    }
}

/// A body already in samples runs on the grid, with its feedback intact.
#[test]
fn a_constant_delay_over_sampled_input_runs_on_the_grid() {
    let g = graph_of(
        "crossed",
        &[(
            "loop",
            "sample(sin(2*pi*220*t)) + 0.5*self[idx(t - 0.01s)]\n",
        )],
    );
    let typing = types(&g, "loop").expect("a recurrence");
    let id = typing.id("loop").expect("the root");
    assert_eq!(typing.ty(id).held, Held::Sampled);
    assert!(reads_itself(&typing, id), "the feedback survives lowering");
}

fn reads_itself(typing: &sva_engine::Typing, id: sva_engine::NodeId) -> bool {
    match typing.value(id) {
        Value::SelfAt { .. } => true,
        Value::Op { args, .. } => args.iter().any(|a| reads_itself(typing, *a)),
        Value::Cast(_, source) | Value::Filter { x: source, .. } | Value::Read { source, .. } => {
            reads_itself(typing, *source)
        }
        Value::ClosedForm(_) | Value::Solver { .. } | Value::Noise(_) => false,
    }
}

/// A body holding a value no term can carry is refused, not truncated.
#[test]
fn a_loop_body_the_series_cannot_carry_refuses() {
    let g = graph_of(
        "uncarried",
        &[
            ("chord", "sin(2*pi*220*t)\n"),
            ("loop", "lowpass(@chord, 800, 0.7) + 0.5*self(t - 0.01s)\n"),
        ],
    );
    let refused = types(&g, "loop").expect_err("no series expands it");
    assert_eq!(refused.code(), "engine.series_body_not_inlinable");
    let EngineError::Refused(d) = &refused else {
        panic!("expected a written refusal");
    };
    let fixed = graph_of(
        "carried",
        &[
            ("chord", "sin(2*pi*220*t)\n"),
            (
                "loop",
                "sample(lowpass(@chord, 800, 0.7)) + 0.5*self[idx(t) - 1]\n",
            ),
        ],
    );
    assert!(
        types(&fixed, "loop").is_ok(),
        "the repair the refusal offers must work: {}",
        d.help
    );
}

/// Two call sites at two delays are two reads of this node's output, never one.
#[test]
fn two_self_reads_at_two_delays_stay_apart() {
    let g = graph_of(
        "two-taps",
        &[(
            "loop",
            "sample(sin(2*pi*220*t)) + 0.4*self[idx(t) - 1] + 0.3*self[idx(t) - 2]\n",
        )],
    );
    let typing = types(&g, "loop").expect("a recurrence");
    let id = typing.id("loop").expect("the root");
    let mut taps = Vec::new();
    collect_taps(&typing, id, &mut taps);
    taps.sort_by_key(|d| format!("{d:?}"));
    taps.dedup_by_key(|d| format!("{d:?}"));
    assert_eq!(taps.len(), 2, "one read per written delay: {taps:?}");
}

fn collect_taps(typing: &sva_engine::Typing, id: sva_engine::NodeId, out: &mut Vec<When>) {
    match typing.value(id) {
        Value::SelfAt { at, .. } => out.push(at.clone()),
        Value::Op { args, .. } => args.iter().for_each(|a| collect_taps(typing, *a, out)),
        Value::Cast(_, source) | Value::Filter { x: source, .. } | Value::Read { source, .. } => {
            collect_taps(typing, *source, out)
        }
        Value::ClosedForm(_) | Value::Solver { .. } | Value::Noise(_) => {}
    }
}

/// A loop reading the sample it is writing has nothing to read.
#[test]
fn a_zero_delay_loop_refuses() {
    let refused = form("zero-delay", "sin(2*pi*220*t) + 0.5*self(t)\n").expect_err("no value");
    assert_eq!(refused.code(), "samples.zero_delay_loop");
}

/// A discrete loop runs on the grid whatever its gain, which is what makes a running sum
/// writable. Only a continuous loop, a geometric series, refuses.
#[test]
fn a_discrete_loop_at_unit_gain_is_a_grid_loop() {
    assert_eq!(
        form(
            "stepped-unit",
            "sample(sin(2*pi*220*t)) + 1.0*self[idx(t) - 1]\n"
        )
        .expect("a running sum"),
        Held::Sampled
    );
}

/// A coefficient of zero is no loop: the body stands as written, with no series around it.
#[test]
fn a_zero_coefficient_leaves_the_body_alone() {
    let g = graph_of(
        "silent",
        &[("loop", "sin(2*pi*220*t) + 0.0*self(t - 0.01s)\n")],
    );
    let typing = types(&g, "loop").expect("a law");
    let id = typing.id("loop").expect("the root");
    assert!(typing.ty(id).has_dual());
    let Value::ClosedForm(form) = typing.value(id) else {
        panic!("a law");
    };
    assert!(
        !matches!(form.body, Body::Series(_)),
        "no series expands a loop that carries nothing"
    );
}

/// Reading ahead is not the same mistake as reading the sample being written.
#[test]
fn a_forward_self_read_names_its_own_cause() {
    let refused = form("forward", "sin(2*pi*220*t) + 0.5*self(t + 0.01s)\n").expect_err("no value");
    assert_eq!(refused.code(), "engine.forward_self_read");
}

/// A forcing term that is itself a series is the common case: every named waveform is one.
#[test]
fn a_series_forcing_term_still_closes_the_loop() {
    let g = graph_of("forced", &[("loop", "saw(220) + 0.5*self(t - 0.01s)\n")]);
    let typing = types(&g, "loop").expect("a series over a series");
    let id = typing.id("loop").expect("the root");
    assert!(typing.ty(id).has_dual());
    let Value::ClosedForm(form) = typing.value(id) else {
        panic!("a law");
    };
    assert!(matches!(form.body, Body::Series(_)));
}

/// The comb the series denotes is one line carrying every term, and the label says how much
/// of the tail the profile's precision left behind.
#[test]
fn a_closed_loop_renders_its_comb_and_reports_its_tail() {
    let g = graph_of(
        "comb",
        &[("loop", "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n")],
    );
    let held = render(&g, "loop", RenderConfig::seconds(8_000, 0.05), None).expect("a comb");
    let id = held.id("loop").expect("the root");
    let buffer = held.output(id).expect("a rendered comb");
    assert_eq!(held.labels[&id].source, Source::Exact);
    let Detail::Lines { terms, tail_db, .. } = &held.labels[&id].detail else {
        panic!(
            "a comb is a line spectrum, got {:?}",
            held.labels[&id].detail
        );
    };
    assert_eq!(*terms, Some(1), "every term lands on the one resonance");
    assert!(
        tail_db.is_some_and(|db| db < 0.0),
        "the truncated tail is stated: {tail_db:?}"
    );
    let peak = buffer.plane(0).iter().fold(0.0f64, |a, s| a.max(s.abs()));
    let closed = 1.0
        / (1.0 - 0.5 * (-std::f64::consts::TAU * 220.0 * 0.01).cos())
            .hypot(0.5 * (-std::f64::consts::TAU * 220.0 * 0.01).sin());
    let off = 20.0 * (peak / closed).log10();
    assert!(
        off < 0.0 && off > tail_db.expect("a stated tail"),
        "the render sits between the closed loop and the tail the label states: {off} dB"
    );
}

/// Four delay lines that read each other are one group, and the group is whatever its
/// members reached: a network fed by samples runs on the grid, seed and all.
#[test]
fn an_fdn_of_sampled_delays_types_as_samples() {
    let g = graph_of(
        "fdn",
        &[
            ("send", "sample(sin(2*pi*220*t))\n"),
            ("a", "@send + 0.45*(self[idx(t) - 1019] + @b(t - 1380sp))\n"),
            ("b", "@send + 0.45*(@a(t - 1019sp) - self[idx(t) - 1380])\n"),
        ],
    );
    let typing = types(&g, "a").expect("a network its own members type");
    for path in ["a", "b"] {
        let id = typing.id(path).unwrap_or_else(|| panic!("{path} typed"));
        assert_eq!(
            typing.ty(id).held,
            Held::Sampled,
            "{path} reads samples around the loop"
        );
    }
}

/// The optimistic seed is still the one a closed form loop settles in.
#[test]
fn a_closed_form_loop_group_keeps_its_dual() {
    let g = graph_of(
        "law-group",
        &[
            ("x", "sin(2*pi*110*t) + 0.4*@y(t - 0.01s)\n"),
            ("y", "sin(2*pi*220*t) + 0.4*@x(t - 0.02s)\n"),
        ],
    );
    let typing = types(&g, "x").expect("a law group");
    for path in ["x", "y"] {
        let id = typing.id(path).unwrap_or_else(|| panic!("{path} typed"));
        assert!(typing.ty(id).has_dual(), "{path} stays a closed form");
    }
}

/// Two nodes each lower a series of their own, and one closed form holds both. An index one of them
/// reused would be captured by the other's expansion, folding two sums into one.
#[test]
fn two_nested_loops_expand_under_indices_of_their_own() {
    let g = graph_of(
        "nested-loops",
        &[
            ("comb", "x + g*self(t - delay)\n"),
            ("inner", "@comb(t, x=1, delay=0.01, g=0.5)\n"),
            ("outer", "@comb(t, x=@inner, delay=0.013, g=0.25)\n"),
        ],
    );
    let held = render(&g, "outer", RenderConfig::seconds(44_100, 0.001), None)
        .expect("two nested Neumann series render");
    let buffer = held
        .output(held.id("outer").expect("the root"))
        .expect("a rendered loop");
    let profile = sva_samples::PSYCHOACOUSTIC_V1;
    let taken = |first: f64, g: f64| {
        (0..)
            .find(|n| first * g.powi(*n) / (1.0 - g) <= profile.half_lsb())
            .expect("a tail under the precision")
    };
    let want: f64 = (0..taken(1.0, 0.5)).map(|j| 0.5f64.powi(j)).sum::<f64>()
        * (0..taken(2.0, 0.25)).map(|k| 0.25f64.powi(k)).sum::<f64>();
    assert!(
        (buffer.at(0, 0) - want).abs() < 1e-12,
        "two truncated geometrics multiply: {} against {want}",
        buffer.at(0, 0)
    );
}

const COMB: &str = "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n";
const SHIFT: f64 = 0.003;

/// A shift of a Neumann series is a shift of each term — an affine change of the free
/// variable, not a new shape — so a shifted comb takes the row of FORMAT 9.1 the unshifted
/// one takes, over the terms the unshifted one took, and its window moves with its voice.
#[test]
fn a_shifted_comb_costs_its_unshifted_route() {
    let routed = |files: &[(&str, &str)]| {
        let g = graph_of("shifted-route", files);
        let held = render(&g, "loop", RenderConfig::seconds(8_000, 0.05), None).expect("a comb");
        let id = held.id("loop").expect("the root");
        let label = held.labels[&id].clone();
        let Detail::Lines { terms, .. } = label.detail else {
            panic!("a comb is a line spectrum, got {:?}", label.detail);
        };
        (label.source, label.rule(), terms)
    };

    let plain = routed(&[("loop", COMB)]);
    assert_eq!(plain.0, Source::Exact);
    let shifted = routed(&[("comb", COMB), ("loop", "@comb(t - 0.003s)\n")]);
    assert_eq!(
        shifted, plain,
        "a shifted series is the same series, placed by the same rule over the same terms"
    );

    let sounding = |files: &[(&str, &str)]| {
        let g = graph_of("shifted-window", files);
        let held = render(&g, "loop", RenderConfig::seconds(8_000, 0.05), None).expect("a comb");
        let id = held.id("loop").expect("the root");
        let plane = held.output(id).expect("a rendered comb").plane(0).to_vec();
        let at = |v: &[f64]| {
            let live: Vec<usize> = v
                .iter()
                .enumerate()
                .filter(|(_, s)| s.abs() > 1e-9)
                .map(|(i, _)| i)
                .collect();
            (live[0], live[live.len() - 1])
        };
        at(&plane)
    };
    let window = [("comb", COMB), ("windowed", "crop(@comb(t), 0s, 0.02s)\n")];
    let (open, close) = sounding(&[window[0], window[1], ("loop", "@windowed(t)\n")]);
    let (moved_open, moved_close) =
        sounding(&[window[0], window[1], ("loop", "@windowed(t - 0.003s)\n")]);
    let steps = (SHIFT * 8_000.0).round() as usize;
    assert_eq!(
        (moved_open, moved_close),
        (open + steps, close + steps),
        "the window a cropped series carries moves with the series, not against it"
    );
}

/// The offset written at the call site, inside the body, or as a phase are three spellings
/// of one rotation: the same lines at the same levels, each turned by exactly `-2*pi*f*by`.
#[test]
fn a_shifted_comb_keeps_its_lines() {
    let listed = |files: &[(&str, &str)]| {
        let g = graph_of("shifted-lines", files);
        let held = render(&g, "loop", RenderConfig::seconds(8_000, 0.05), None).expect("a comb");
        let id = held.id("loop").expect("the root");
        let found = answer(&held, id, Representation::Lines).expect("an exact line list");
        assert_eq!(found.source, Source::Exact);
        let Output::Lines(mut lines) = found.value else {
            panic!("expected lines");
        };
        lines.sort_by(|a, b| a.hz.total_cmp(&b.hz));
        lines
    };

    let plain = listed(&[("loop", COMB)]);
    assert!(plain.len() > 2, "a comb is more than its excitation");

    for files in [
        vec![("comb", COMB), ("loop", "@comb(t - 0.003s)\n")],
        vec![("loop", "sin(2*pi*220*(t - 0.003)) + 0.5*self(t - 0.01s)\n")],
        vec![(
            "loop",
            "sin(2*pi*220*t - 2*pi*220*0.003) + 0.5*self(t - 0.01s)\n",
        )],
    ] {
        let moved = listed(&files);
        assert_eq!(moved.len(), plain.len(), "{files:?}");
        for (was, is) in plain.iter().zip(&moved) {
            assert!(
                (is.hz - was.hz).abs() < 1e-9,
                "{files:?}: {is:?} vs {was:?}"
            );
            let turn = sva_engine::C64::new(0.0, -std::f64::consts::TAU * was.hz * SHIFT).exp();
            let want = was.amp * turn;
            assert!(
                (is.amp - want).abs() < 1e-9,
                "{files:?}: {} Hz turned to {:?}, not {want:?}",
                was.hz,
                is.amp
            );
        }
    }
}
