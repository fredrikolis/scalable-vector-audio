// Concern: proves a loop is classified as a series or as a sampled loop by what its members are | Non-concern: running either (sva-samples) | IO: (a composition) -> Ty or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::{
    Delay, Detail, EngineError, Held, Output, RenderConfig, Representation, Source, Value, answer,
    render, types,
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
fn an_sp_delayed_self_is_discrete() {
    assert_eq!(
        form("sampled", "sample(sin(2*pi*220*t)) + 0.5*self(t - 1sp)\n").expect("a recurrence"),
        Held::Sampled,
        "the sp unit puts the loop on the observation's grid"
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
        d.help.contains("1sp"),
        "the sampled loop is offered: {}",
        d.help
    );

    let above = form("above", "sin(2*pi*220*t) + 1.5*self(t - 0.01s)\n").expect_err("no value");
    assert_eq!(above.code(), "type.self_gain_unbounded");
}

#[test]
fn a_modulated_delay_is_sampled() {
    assert_eq!(
        form(
            "modulated",
            "sample(sin(2*pi*220*t)) + 0.5*self(t - (0.01s + 0.002s*sin(2*pi*3*t)))\n"
        )
        .expect("a recurrence"),
        Held::Sampled,
        "a delay that moves cannot be one shift of a series"
    );
}

/// The other row of FORMAT 11: no series expands a nonlinearity.
#[test]
fn a_nonlinear_self_is_sampled() {
    assert_eq!(
        form(
            "nonlinear",
            "sample(sin(2*pi*220*t)) + tanh(self(t - 0.01s)*2)\n"
        )
        .expect("a recurrence"),
        Held::Sampled
    );
}

/// A body already in samples runs on the grid, with its feedback intact.
#[test]
fn a_constant_delay_over_sampled_input_runs_on_the_grid() {
    let g = graph_of(
        "crossed",
        &[("loop", "sample(sin(2*pi*220*t)) + 0.5*self(t - 0.01s)\n")],
    );
    let typing = types(&g, "loop").expect("a recurrence");
    let id = typing.id("loop").expect("the root");
    assert_eq!(typing.ty(id).held, Held::Sampled);
    assert!(reads_itself(&typing, id), "the feedback survives lowering");
}

fn reads_itself(typing: &sva_engine::Typing, id: sva_engine::NodeId) -> bool {
    match typing.value(id) {
        Value::SelfAt(_) => true,
        Value::Op { args, .. } => args.iter().any(|a| reads_itself(typing, *a)),
        Value::Cast(_, source) | Value::Filter { x: source, .. } | Value::Read { source, .. } => {
            reads_itself(typing, *source)
        }
        Value::ClosedForm(_) | Value::Solver(_) | Value::Grid(_) => false,
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
                "sample(lowpass(@chord, 800, 0.7)) + 0.5*self(t - 1sp)\n",
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
            "sample(sin(2*pi*220*t)) + 0.4*self(t - 1sp) + 0.3*self(t - 2sp)\n",
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

fn collect_taps(typing: &sva_engine::Typing, id: sva_engine::NodeId, out: &mut Vec<Delay>) {
    match typing.value(id) {
        Value::SelfAt(delay) => out.push(*delay),
        Value::Op { args, .. } => args.iter().for_each(|a| collect_taps(typing, *a, out)),
        Value::Cast(_, source) | Value::Filter { x: source, .. } | Value::Read { source, .. } => {
            collect_taps(typing, *source, out)
        }
        Value::ClosedForm(_) | Value::Solver(_) | Value::Grid(_) => {}
    }
}

/// A loop reading the sample it is writing has nothing to read.
#[test]
fn a_zero_delay_loop_refuses() {
    let refused = form("zero-delay", "sin(2*pi*220*t) + 0.5*self(t)\n").expect_err("no value");
    assert_eq!(refused.code(), "samples.zero_delay_loop");
}

/// FORMAT 11 row three: a delay in `sp` runs on the grid whatever its gain, which is what
/// makes a running sum writable. Only the seconds row, a geometric series, refuses.
#[test]
fn a_stepped_delay_at_unit_gain_is_a_grid_loop() {
    assert_eq!(
        form(
            "stepped-unit",
            "sample(sin(2*pi*220*t)) + 1.0*self(t - 1sp)\n"
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

/// A grid offset is a whole number of samples; rounding one would move the read silently.
#[test]
fn a_fractional_grid_delay_refuses() {
    let refused = form(
        "fractional",
        "sample(sin(2*pi*220*t)) + 0.5*self(t - 1.5sp)\n",
    )
    .expect_err("no value");
    assert_eq!(refused.code(), "ref.fractional_shift_on_samples");
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
/// of the tail the profile's floor left behind.
#[test]
fn a_closed_loop_renders_its_comb_and_reports_its_tail() {
    let g = graph_of(
        "comb",
        &[("loop", "sin(2*pi*220*t) + 0.5*self(t - 0.01s)\n")],
    );
    let held = render(&g, "loop", RenderConfig::seconds(8_000, 0.05), None).expect("a comb");
    let id = held.id("loop").expect("the root");
    let buffer = held.buffer(id).expect("a rendered comb");
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
            ("a", "@send + 0.45*(self(t - 1019sp) + @b(t - 1380sp))\n"),
            ("b", "@send + 0.45*(@a(t - 1019sp) - self(t - 1380sp))\n"),
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
        .buffer(held.id("outer").expect("the root"))
        .expect("a rendered loop");
    let profile = sva_samples::PSYCHOACOUSTIC_V1;
    let floor = 10f64.powf(profile.floor(profile.ceiling(44_100)) / 20.0);
    let taken = |g: f64| (0..).find(|n| g.powi(*n) < floor).expect("a floor");
    let want: f64 = (0..taken(0.5)).map(|j| 0.5f64.powi(j)).sum::<f64>()
        * (0..taken(0.25)).map(|k| 0.25f64.powi(k)).sum::<f64>();
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
        let plane = held.buffer(id).expect("a rendered comb").plane(0).to_vec();
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
