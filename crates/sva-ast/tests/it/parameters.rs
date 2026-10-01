// Concern: proves which parameters one node reads that no default line of its own binds | Non-concern: what a caller binds them to (sva-engine) | IO: (node text) -> names

use sva_ast::{Composition, free_parameters, load, parse_file};

/// One node's text, with no node beside it.
fn alone(text: &str) -> Vec<String> {
    let parsed = parse_file("tone", text).unwrap_or_else(|d| panic!("{text}: {}", d.message));
    free_parameters(&parsed.expr, &parsed.defaults, &[], |_| false)
}

#[test]
fn a_parameter_with_a_default_line_is_bound() {
    assert_eq!(alone("f0 = 440\nsin(2*pi*f0*t)\n"), [] as [&str; 0]);
    assert_eq!(alone("sin(2*pi*f0*t)\n"), ["f0"]);
}

#[test]
fn each_is_named_once_body_first_then_the_default_lines() {
    assert_eq!(
        alone("g = base*2\nvel*sin(2*pi*f0*t)*vel + f0*g\n"),
        ["vel", "f0", "base"]
    );
}

#[test]
fn a_default_line_sees_only_the_lines_above_it() {
    assert_eq!(alone("base = 1\nf0 = base*2\nsin(f0*t)\n"), [] as [&str; 0]);
    assert_eq!(alone("f0 = base*2\nbase = 1\nsin(f0*t + base)\n"), ["base"]);
}

#[test]
fn the_languages_own_names_are_no_parameters() {
    let text = "gain = -6db\ncrop(lowpass(sample(sin(2*pi*A4*t) + sum(k, 1, 3, sin(2*pi*k*C4*t)/k)), \
                cutoff=800hz, q=0.7)*gain + 0.5*self[idx(t) - 1] + f*i*pi, 0s, inf)\n";
    assert_eq!(alone(text), [] as [&str; 0]);
}

#[test]
fn a_name_shaped_like_a_note_beyond_midi_is_a_parameter() {
    assert_eq!(alone("sin(H4*t) + sin(G10*t)\n"), ["H4", "G10"]);
}

#[test]
fn a_call_or_ref_argument_value_is_read_and_its_key_is_not() {
    assert_eq!(alone("crop(sin(t), 0s, release)\n"), ["release"]);
    assert_eq!(alone("crop(sin(t), end=release)\n"), ["release"]);
    assert_eq!(alone("@voice(t, f0=pitch)\n"), ["pitch"]);
}

#[test]
fn a_parameter_read_by_index_or_called_at_a_time_is_a_parameter() {
    assert_eq!(alone("x[idx(t) - 1]\n"), ["x"]);
    assert_eq!(alone("x(t - 1ms)\n"), ["x"]);
    assert_eq!(alone("x = 0\nx(t - 1ms)\n"), [] as [&str; 0]);
}

#[test]
fn a_name_the_caller_binds_is_bound_and_its_default_line_is_not_read() {
    let parsed = parse_file("tone", "f0 = base*2\namp*sin(f0*t)\n").expect("parses");
    let free = |given: &[&str]| free_parameters(&parsed.expr, &parsed.defaults, given, |_| false);
    assert_eq!(free(&[]), ["amp", "base"]);
    assert_eq!(free(&["amp", "f0"]), [] as [&str; 0]);
    assert_eq!(free(&["base"]), ["amp"]);
}

#[test]
fn a_series_index_is_bound_only_in_its_own_term() {
    assert_eq!(alone("sum(k, 1, 4, sin(k*t))\n"), [] as [&str; 0]);
    assert_eq!(alone("sum(k, 1, 4, sin(k*t)) * k\n"), ["k"]);
    assert_eq!(alone("sum(k, 1, k, sin(k*t))\n"), ["k"]);
}

#[test]
fn a_bareword_call_naming_a_node_invokes_it() {
    let graph = load(&Composition::from_iter([
        ("comb", "x = 0\ndelay = 0.01\nx + 0.5*x(t - delay)\n"),
        ("song", "comb(sin(t), delay=d) + echo(t)\n"),
    ]))
    .unwrap_or_else(|r| panic!("{r:?}"));
    assert_eq!(graph.free_parameters("song"), ["d", "echo"]);
    assert_eq!(graph.free_parameters("comb"), [] as [&str; 0]);
}
