// Concern: proves an argument reaches its slot as the number its text names | Non-concern: what a builtin does with the number (vocabulary.rs) | IO: (a composition) -> the number that arrived

mod fixtures;

use fixtures::graph_of;
use sva_engine::{EngineError, Held, RenderConfig, Value, Var, render, types};
use sva_formula::Body;

fn form(name: &str, body: &str) -> (Held, bool) {
    let g = graph_of(name, &[("body", body)]);
    let typing = types(&g, "body").expect("a composition that types");
    let id = typing.id("body").expect("the root");
    (typing.ty(id).held, typing.ty(id).dual)
}

fn peak(name: &str, body: &str) -> f64 {
    let g = graph_of(name, &[("body", body)]);
    let held = render(&g, "body", RenderConfig::seconds(44_100, 0.05), None).expect("a render");
    let root = held.id("body").expect("the root");
    held.buffer(root)
        .expect("a rendered law")
        .plane(0)
        .iter()
        .fold(0f64, |so_far, s| so_far.max(s.abs()))
}

fn drawn(name: &str, body: &str) -> f64 {
    let g = graph_of(name, &[("body", body)]);
    let typing = types(&g, "body").expect("a keyed hash types");
    let id = typing.id("body").expect("the root");
    let Value::ClosedForm(form) = typing.value(id) else {
        panic!("a keyed hash of a constant key is a law");
    };
    match &form.body {
        Body::Const(c) => c.re,
        other => panic!("a constant seed draws one number, not {other:?}"),
    }
}

/// A dropped argument is replaced by the builtin's default, silently.
#[test]
fn a_modulo_inside_a_constant_argument_folds() {
    let written = "bandpass(sin(2*pi*300*t), cutoff=1450 + 130*(5 % 4), q=1.15)\n";
    assert_eq!(
        form("modulo-cutoff", written),
        (Held::Form(Var::T), true),
        "every argument is constant, so the filter keeps its dual"
    );
    let folded = peak("modulo-peak", written);
    let spelled = peak(
        "literal-peak",
        "bandpass(sin(2*pi*300*t), cutoff=1580, q=1.15)\n",
    );
    assert!(
        (folded - spelled).abs() < 1e-12,
        "1450 + 130*(5 % 4) is 1580: {folded} against {spelled}"
    );
}

#[test]
fn a_modulo_inside_a_seed_is_not_dropped() {
    let written = drawn("modulo-seed", "rand(seed=311 % 256)\n");
    assert_eq!(
        written,
        drawn("literal-seed", "rand(seed=55)\n"),
        "311 % 256 is 55"
    );
    assert_ne!(
        written,
        drawn("fallback-seed", "rand(seed=0)\n"),
        "the seed a `%` spells is not the seed a missing argument falls back to"
    );
}

#[test]
fn a_called_operator_inside_a_constant_argument_folds() {
    let folded = peak(
        "called-peak",
        "bandpass(sin(2*pi*300*t), cutoff=pow(2, 3)*max(190, 12) + abs(-60), q=min(1.15, 4))\n",
    );
    let spelled = peak(
        "called-literal",
        "bandpass(sin(2*pi*300*t), cutoff=1580, q=1.15)\n",
    );
    assert!(
        (folded - spelled).abs() < 1e-12,
        "pow(2, 3)*max(190, 12) + abs(-60) is 1580: {folded} against {spelled}"
    );
}

#[test]
fn a_constant_that_names_no_number_refuses() {
    for written in [
        "sin(2*pi*300*t) * (5 % 0)\n",
        "sin(2*pi*300*t) * (1/0)\n",
        "sin(2*pi*300*t) * log(0)\n",
        "sin(2*pi*300*t) * exp(710)\n",
    ] {
        let g = graph_of("no-number", &[("body", written)]);
        let Err(refused) = render(&g, "body", RenderConfig::seconds(44_100, 0.01), None) else {
            panic!("{written} names no number and renders nothing");
        };
        let EngineError::Refused(d) = &refused else {
            panic!("{written} is refused in writing, not {refused:?}");
        };
        assert!(
            d.message.contains("no number") || d.message.contains("no value"),
            "{written}: {}",
            d.message
        );
    }
}

#[test]
fn an_argument_that_names_no_number_refuses() {
    let g = graph_of(
        "no-number-argument",
        &[(
            "body",
            "bandpass(sin(2*pi*300*t), cutoff=1450 + log(0), q=1.15)\n",
        )],
    );
    types(&g, "body").expect_err("a cutoff that names no number is no cutoff");
}

/// FORMAT 15.3: a ref naming one number is that number wherever it is written. A window's
/// bounds are two numbers, so a constant expression folds into one; only a bound that moves
/// with the free variable names no window.
#[test]
fn a_crop_bound_may_be_a_constant_expression() {
    let g = graph_of(
        "crop-bound",
        &[
            ("variables/bar", "0.5\n"),
            ("tone", "sin(2*pi*440*t)\n"),
            ("written", "crop(@tone(t), 0s, 1.6s - 0.1s)\n"),
            ("read", "crop(@tone(t), 0s, @variables/bar*3)\n"),
        ],
    );
    let ends = |node: &str| {
        let held = render(&g, node, RenderConfig::seconds(8_192, 2.0), None)
            .unwrap_or_else(|e| panic!("{node}: {e}"));
        let id = held.id(node).unwrap_or_else(|| panic!("{node} typed"));
        let plane = held.buffer(id).expect("a rendered law").plane(0).to_vec();
        let last = plane.iter().rposition(|s| *s != 0.0).expect("some sound");
        (last + 1) as f64 / 8_192.0
    };
    assert!(
        (ends("written") - 1.5).abs() < 1e-3,
        "1.6s - 0.1s is 1.5s, got {}",
        ends("written")
    );
    assert!(
        (ends("read") - 1.5).abs() < 1e-3,
        "a bar of 0.5s three times over is 1.5s, got {}",
        ends("read")
    );
}

/// A filter argument is one of those slots: `cutoff=@variables/key` names the same number
/// the literal does, and the two nodes render the same samples.
#[test]
fn a_filter_cutoff_may_be_a_scalar_variable() {
    let g = graph_of(
        "filter-cutoff-ref",
        &[
            ("variables/key", "440\n"),
            ("tone", "sin(2*pi*220*t)\n"),
            ("written", "bandpass(@tone(t), cutoff=440, q=1)\n"),
            ("read", "bandpass(@tone(t), cutoff=@variables/key, q=1)\n"),
        ],
    );
    let plane = |node: &str| {
        let held = render(&g, node, RenderConfig::seconds(8_192, 0.05), None)
            .unwrap_or_else(|e| panic!("{node}: {e}"));
        let id = held.id(node).unwrap_or_else(|| panic!("{node} typed"));
        held.buffer(id).expect("a rendered law").plane(0).to_vec()
    };
    assert_eq!(
        plane("read"),
        plane("written"),
        "a scalar ref in a filter argument is the number it names"
    );
}

fn plane_of(g: &sva_ast::Graph, node: &str) -> Vec<f64> {
    let held = render(g, node, RenderConfig::seconds(44_100, 0.02), None)
        .unwrap_or_else(|e| panic!("{node}: {e}"));
    let id = held.id(node).unwrap_or_else(|| panic!("{node} typed"));
    held.buffer(id).expect("a rendered solve").plane(0).to_vec()
}

/// The bug this closes: a negative whole exponent of a constant lowered to a pole atom, which
/// no fold reads as a number, so `b=` fell back to its default for every such exponent.
#[test]
fn a_negative_whole_power_inside_a_solver_argument_is_the_number_it_names() {
    let x = 261.63_f64 / 262.0;
    // `C64::inv` of a real `k`, the reciprocal every pole weight and quotient takes.
    let inv = |k: f64| k / (k * k);
    for (exponent, reciprocal) in [
        ("-1", inv(x)),
        ("-1.0", inv(x)),
        ("-2", inv(x * x)),
        ("-3", inv(x * x * x)),
    ] {
        let g = graph_of(
            "negative-power",
            &[
                (
                    "powered",
                    &format!(
                        "f0 = 261.63\nchaigne_askenfelt(f0, b=0.0001*pow(f0/262, {exponent}))\n"
                    ),
                ),
                (
                    "spelled",
                    &format!("chaigne_askenfelt(261.63, b=0.0001*{reciprocal:?})\n"),
                ),
                ("omitted", "chaigne_askenfelt(261.63)\n"),
            ],
        );
        let powered = plane_of(&g, "powered");
        assert_eq!(
            powered,
            plane_of(&g, "spelled"),
            "pow(f0/262, {exponent}) is {reciprocal:?}"
        );
        assert_ne!(
            powered,
            plane_of(&g, "omitted"),
            "pow(f0/262, {exponent}) is not the default b"
        );
    }
}

#[test]
fn a_negative_whole_power_of_zero_names_no_number() {
    let g = graph_of("zero-power", &[("body", "sin(2*pi*300*t) * pow(0, -2)\n")]);
    let Err(refused) = render(&g, "body", RenderConfig::seconds(44_100, 0.01), None) else {
        panic!("0^-2 names no number and renders nothing");
    };
    assert!(
        matches!(&refused, EngineError::Refused(d) if d.message.contains("no number") || d.message.contains("no value")),
        "{refused:?}"
    );
}
