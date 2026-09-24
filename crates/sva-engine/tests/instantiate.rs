// Concern: proves one instance exists per argument tuple and names what a binding can be refused for | Non-concern: ordering the instances (refs.rs) | IO: (a composition) -> Instances or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::instantiate::{Instances, SIGNAL_PARAM, from_roots, instantiate};
use sva_engine::{BindingFault, EngineError};

fn fault_of(e: EngineError) -> BindingFault {
    match e {
        EngineError::Binding { fault, .. } => fault,
        other => panic!("expected a binding refusal, got {other:?}"),
    }
}

fn named(i: &Instances) -> Vec<String> {
    i.paths().map(str::to_string).collect()
}

/// A file with no free variables is a plain node under its own path, exactly as before.
#[test]
fn an_unparameterized_composition_instantiates_under_its_own_paths() {
    let g = graph_of("plain", &[("kick", "sin(t)\n"), ("song", "@kick*0.5\n")]);
    let i = instantiate(&g, "song").unwrap();
    assert_eq!(
        named(&i),
        vec!["kick", "song"],
        "no invocation, no new names"
    );
    assert_eq!(i.origin("kick"), Some("kick"));
}

#[test]
fn each_argument_tuple_is_its_own_instance_and_equal_tuples_share_one() {
    let g = graph_of(
        "tuples",
        &[
            ("src", "sin(t)\n"),
            ("gain", "x*k\n"),
            (
                "song",
                "@gain(t, x=@src, k=2) + @gain(t, x=@src, k=3) + @gain(t, k=2, x=@src)\n",
            ),
        ],
    );
    let i = instantiate(&g, "song").unwrap();
    assert_eq!(
        named(&i),
        vec!["gain(k=2, x=@src)", "gain(k=3, x=@src)", "song", "src"],
        "two tuples, three call sites: the repeat shares, and argument order does not matter"
    );
}

/// The whole point of naming instances rather than files: nothing about `k=2` may be
/// reachable from `k=3`'s buffer. A second root whose own file is spelled like an instance
/// name is what makes two different tuples compete for one name.
#[test]
fn a_name_a_different_tuple_already_holds_takes_the_next_suffix() {
    let g = graph_of(
        "collide",
        &[
            ("f", "k\n"),
            ("f(k=2)", "1\n"),
            ("song", "@f(t, k=1) + @f(t, k=2)\n"),
        ],
    );
    let (i, roots) =
        from_roots(&g, &["f(k=2)".to_string(), "song".to_string()]).expect("two roots");
    assert_eq!(roots, vec!["f(k=2)".to_string(), "song".to_string()]);
    assert_eq!(i.origin("f(k=2)"), Some("f(k=2)"), "the file took the name");
    assert!(
        i.holds("f(k=2)~2"),
        "the tuple k=2 took the next suffix: {:?}",
        named(&i)
    );
    assert_eq!(i.origin("f(k=2)~2"), Some("f"));
    assert!(i.holds("f(k=1)"));
}

#[test]
fn an_unbound_variable_and_an_unused_argument_both_refuse() {
    let g = graph_of(
        "unbound",
        &[
            ("f", "x*k\n"),
            ("src", "sin(t)\n"),
            ("song", "@f(t, x=@src)\n"),
        ],
    );
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::Unbound("f".to_string(), "k".to_string())
    );

    let g = graph_of(
        "unused",
        &[
            ("f", "x*2\n"),
            ("src", "sin(t)\n"),
            ("song", "@f(t, x=@src, gain=3)\n"),
        ],
    );
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::Unused("f".to_string(), "gain".to_string())
    );
}

/// Where the fix is: the caller omitted the argument, so the caller's file and the span of
/// the invocation are what a reader needs, not the callee's body.
#[test]
fn a_missing_argument_is_located_at_the_invocation_rather_than_at_the_free_variable() {
    let g = graph_of(
        "located",
        &[
            ("src", "sin(t)\n"),
            ("fx/gain", "x*k\n"),
            ("song", "0.5 + @fx/gain(t, x=@src)\n"),
        ],
    );
    let EngineError::Binding { node, span, fault } = instantiate(&g, "song").unwrap_err() else {
        panic!("expected a binding refusal")
    };
    assert_eq!(node, "song");
    assert_eq!(span.map(|s| (s.start, s.end)), Some((6, 14)));
    assert_eq!(
        fault,
        BindingFault::Unbound("fx/gain".to_string(), "k".to_string())
    );
}

/// A series binds its index over its term alone, so `k` is neither a parameter the caller
/// must pass nor an unbound name.
#[test]
fn the_free_names_and_a_series_index_bind_without_an_argument() {
    let g = graph_of(
        "freenames",
        &[(
            "song",
            "sum(k, 1, inf, sin(2*pi*k*220*t)/k) + exp(i*2*pi*f) + pv(f) + delta(t, k=1)\n",
        )],
    );
    assert!(instantiate(&g, "song").is_ok());

    let loose = graph_of("freenames-loose", &[("song", "sin(k*t)\n")]);
    assert_eq!(
        fault_of(instantiate(&loose, "song").unwrap_err()),
        BindingFault::Unbound("song".to_string(), "k".to_string()),
        "an index is bound by its own series, not by the file"
    );
}

#[test]
fn a_parameter_may_not_take_a_name_the_language_already_binds() {
    let g = graph_of(
        "reserved",
        &[("f", "x*2\n"), ("song", "@f(t, x=1, sin=2)\n")],
    );
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::Reserved("sin".to_string())
    );
}

/// Both refusals guard the same thing: an argument whose value depends on which sample is
/// being written cannot be moved into a callee that reads it somewhere else.
#[test]
fn a_self_argument_and_a_shifted_read_of_a_stateful_one_refuse() {
    let g = graph_of(
        "selfarg",
        &[("f", "x*2\n"), ("song", "@f(t, x=self(t - 1sp))\n")],
    );
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::SelfInArgument("x".to_string())
    );

    let g = graph_of(
        "shifted",
        &[
            ("f", "x(t - 0.01s)\n"),
            ("src", "sin(t)\n"),
            ("song", "@f(t, x=lp(@src, 800))\n"),
        ],
    );
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::ShiftedRead("x".to_string(), "lp".to_string())
    );
}

#[test]
fn a_chained_or_plain_call_to_a_file_binds_its_receiver_to_the_signal_parameter() {
    let g = graph_of(
        "chain",
        &[
            ("src", "sin(t)\n"),
            ("gain", "x*k\n"),
            ("song", "@src.gain(k=2) + gain(@src, k=2)\n"),
        ],
    );
    let i = instantiate(&g, "song").unwrap();
    assert!(
        i.holds("gain(k=2, x=@src)"),
        "both spellings reach one instance: {:?}",
        named(&i)
    );
    assert_eq!(SIGNAL_PARAM, "x");
}

#[test]
fn a_bareword_invocation_binding_one_name_twice_refuses_as_a_duplicate() {
    let g = graph_of(
        "duplicate",
        &[
            ("src", "sin(t)\n"),
            ("f", "x*k\n"),
            ("song", "f(@src, k=1, k=2)\n"),
        ],
    );
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::Duplicate("k".to_string())
    );
}

#[test]
fn a_second_positional_argument_refuses_rather_than_guessing_an_order() {
    let g = graph_of(
        "positional",
        &[
            ("src", "sin(t)\n"),
            ("f", "x*k\n"),
            ("song", "f(@src, 2)\n"),
        ],
    );
    assert!(matches!(
        instantiate(&g, "song"),
        Err(EngineError::BadArity(_))
    ));
}

/// Two names of a length can share a lookup key, and each must still find its own.
#[test]
fn two_parameters_sharing_seven_bytes_and_a_length_each_answer() {
    let g = graph_of(
        "collide7",
        &[
            ("f", "abcdefg1*10 + abcdefg2\n"),
            ("song", "@f(t, abcdefg1=2, abcdefg2=3)\n"),
        ],
    );
    let i = instantiate(&g, "song").unwrap();
    let copies = i.exprs();
    assert_eq!(
        sva_ast::render_expr(&copies["f(abcdefg1=2, abcdefg2=3)"]),
        "2*10 + 3",
        "each name found its own binding"
    );
}

#[test]
fn bindings_holds_the_override_a_default_never_reaches() {
    let g = graph_of(
        "bindings",
        &[
            ("src", "sin(t)\n"),
            ("f", "k = 1\nx*k\n"),
            ("song", "@f(t, x=@src, k=2)\n"),
        ],
    );
    let i = instantiate(&g, "song").unwrap();
    let path = i.paths().find(|p| p.starts_with("f(")).unwrap().to_string();
    let vars: Vec<(String, String)> = i
        .bindings(&path)
        .unwrap()
        .into_iter()
        .map(|(name, e, cx)| (name.to_string(), i.render(e, cx)))
        .collect();
    assert_eq!(
        vars,
        vec![
            ("k".to_string(), "2".to_string()),
            ("x".to_string(), "@src".to_string()),
        ],
        "k's default is 1; the call site's override must be what shows"
    );
    assert!(i.bindings("nope").is_none());
}

/// A parameter's expression is never copied, so what a walk sees has to be assembled: this
/// pins the assembly against the substituted tree it replaces, shifts and all.
#[test]
fn a_resolved_walk_reads_as_the_substituted_copy_did() {
    let g = graph_of(
        "resolved",
        &[
            ("src", "sin(t)\n"),
            ("inner", "w(t - 0.02s)*2\n"),
            ("outer", "@inner(t, w=z + 1)\n"),
            ("song", "@outer(t, z=@src(t)*3)\n"),
        ],
    );
    let i = instantiate(&g, "song").unwrap();
    let copies = i.exprs();
    let printed: Vec<String> = copies
        .iter()
        .map(|(path, e)| format!("{path} = {}", sva_ast::render_expr(e)))
        .collect();
    assert_eq!(
        printed,
        vec![
            "inner(w=@src*3 + 1) = (@src(t - 0.02s)*3 + 1)*2",
            "outer(z=@src*3) = @inner(w=@src*3 + 1)",
            "song = @outer(z=@src*3)",
            "src = sin(t)",
        ],
        "a shifted parameter read moves every `t` under it, however many scopes deep"
    );
}

/// A default is a parameter, so a builtin's name is no more available to one than to a bind.
#[test]
fn a_default_may_not_take_a_name_the_language_already_binds() {
    let g = graph_of("shadowed", &[("song", "crop = 5\ncrop(sin(t), 0s, 1s)\n")]);
    assert_eq!(
        fault_of(instantiate(&g, "song").unwrap_err()),
        BindingFault::Reserved("crop".to_string())
    );
}

/// A default resolves with no invocation behind it, so what it *names* has to settle to a
/// number. What it is written out as does not: a term longhand carries its own meaning, down
/// to one a finite-difference builtin puts on the grid.
#[test]
fn a_default_may_hold_a_written_signal_and_may_not_name_one() {
    for body in [
        "sin(2*pi*440*t)",
        "sample(sin(2*pi*440*t))",
        "chaigne_askenfelt(440)",
    ] {
        let g = graph_of(
            "default-written-longhand",
            &[("song", &format!("held = {body}\nheld*0.5\n"))],
        );
        assert!(
            instantiate(&g, "song").is_ok(),
            "`{body}` is written out, so no invocation has to resolve it"
        );
    }

    let g = graph_of(
        "default-names-a-signal",
        &[
            ("tone", "sin(2*pi*440*t)\n"),
            ("song", "held = @tone\nheld*0.5\n"),
        ],
    );
    let refused = instantiate(&g, "song").expect_err("a named signal is no number");
    assert_eq!(refused.code(), "engine.default_reads_buffer");
}

/// FORMAT 15.3 gives `f0=@variables/key*3st` as its own example. A ref naming one number is
/// that number in a default too; a ref naming a signal is what a caller passes.
#[test]
fn a_default_may_read_a_scalar_variable() {
    let g = graph_of(
        "default-reads-a-scalar",
        &[
            ("variables/key", "146.8324\n"),
            ("partial", "f0 = @variables/key*3st\nsin(2*pi*f0*t)\n"),
            ("master", "@partial(t)\n"),
        ],
    );
    let held = instantiate(&g, "master").expect("a scalar ref folds in a default");
    assert!(
        held.paths().any(|p| p.starts_with("partial(")),
        "one instance, its default resolved: {:?}",
        held.paths().collect::<Vec<_>>()
    );

    let g = graph_of(
        "default-reads-a-law",
        &[
            ("tone", "sin(2*pi*440*t)\n"),
            ("voice", "f0 = @tone\nsin(2*pi*f0*t)\n"),
            ("master", "@voice(t)\n"),
        ],
    );
    let refused = instantiate(&g, "master").expect_err("a signal is not a default");
    assert_eq!(refused.code(), "engine.default_reads_buffer");

    let g = graph_of(
        "default-reads-no-number",
        &[
            (
                "nothing", "1 / 0
",
            ),
            (
                "voice",
                "f0 = @nothing*2
sin(2*pi*f0*t)
",
            ),
            (
                "master",
                "@voice(t)
",
            ),
        ],
    );
    let refused = instantiate(&g, "master").expect_err("a quotient at zero is no number");
    assert_eq!(refused.code(), "engine.default_reads_buffer");
}

/// The names above a default are part of its arithmetic, resolved in declaration order
/// against what the caller bound.
#[test]
fn a_default_may_read_a_bound_default() {
    let files = &[
        ("motif", "key = 200\nthird = key*2\nsin(2*pi*third*t)\n"),
        ("master", "@motif(t) + @motif(t, key=300)\n"),
    ];
    let g = graph_of("chained-defaults", files);
    let held = instantiate(&g, "master").expect("a default reads the line above it");

    let mut named: Vec<String> = held
        .paths()
        .filter(|p| p.starts_with("motif("))
        .map(str::to_string)
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec![
            "motif(key=200, third=200*2)".to_string(),
            "motif(key=300, third=300*2)".to_string()
        ],
        "the caller's key reaches the default written off it, one instance each"
    );
}

/// A series index is its own name inside the body, so a default written as a series over the
/// parameter's name does not read the bound parameter.
#[test]
fn a_series_index_shadows_a_bound_default() {
    let files = &[
        (
            "motif",
            "key = 200\nharm = sum(key, 1, 3, key)\nsin(2*pi*key*harm*t)\n",
        ),
        ("master", "@motif(t) + @motif(t, key=300)\n"),
    ];
    let g = graph_of("series-shadows-default", files);
    let held = instantiate(&g, "master").expect("a series over a parameter's name");

    let mut named: Vec<String> = held
        .paths()
        .filter(|p| p.starts_with("motif("))
        .map(str::to_string)
        .collect();
    named.sort();
    assert_eq!(
        named,
        vec![
            "motif(harm=sum(key, 1, 3, key), key=200)".to_string(),
            "motif(harm=sum(key, 1, 3, key), key=300)".to_string()
        ],
        "the index is never replaced by the caller's key"
    );
}
