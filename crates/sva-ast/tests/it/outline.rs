// Concern: proves an outline is the parser's own tree and each span names the bytes a node was written in | Non-concern: what a node means (sva-engine) | IO: (&str) -> Outline

use sva_ast::outline::{Argument, Form};
use sva_ast::{Arg, ByteSpan, Expr, Outline, outline, parse_expr};

fn text(src: &str, at: ByteSpan) -> &str {
    &src[at.start..at.end]
}

/// The tree an outline stands for, spans dropped, so it compares against `parse_expr`'s.
fn expr_of(o: &Outline) -> Expr {
    let at = ByteSpan::at(0);
    match &o.form {
        Form::Literal(l) => Expr::Lit(l.clone()),
        Form::Name(n) => Expr::Var(n.clone()),
        Form::Operator {
            op, left, right, ..
        } => Expr::Bin(*op, Box::new(expr_of(left)), Box::new(expr_of(right))),
        Form::Call { name, args, .. } => Expr::Call {
            name: name.clone(),
            args: args.iter().map(arg_of).collect(),
            span: at,
        },
        Form::Ref {
            path, arg, binds, ..
        } => Expr::Ref {
            path: path.clone(),
            arg: Box::new(expr_of(arg)),
            binds: binds
                .iter()
                .map(|b| match arg_of(b) {
                    Arg::Named(k, v) => (k, v),
                    Arg::Pos(_) => unreachable!("a ref binds by name"),
                })
                .collect(),
            span: at,
        },
        Form::SelfRef { arg, .. } => Expr::SelfRef {
            arg: Box::new(expr_of(arg)),
            span: at,
        },
    }
}

fn arg_of(a: &Argument) -> Arg {
    match a {
        Argument::Positional(x) => Arg::Pos(expr_of(x)),
        Argument::Named { name, value, .. } => Arg::Named(name.clone(), expr_of(value)),
    }
}

fn value(a: &Argument) -> &Outline {
    match a {
        Argument::Positional(x) | Argument::Named { value: x, .. } => x,
    }
}

fn children(o: &Outline) -> Vec<&Outline> {
    match &o.form {
        Form::Literal(_) | Form::Name(_) => Vec::new(),
        Form::Operator { left, right, .. } => vec![left.as_ref(), right.as_ref()],
        Form::Call { args, .. } => args.iter().map(value).collect(),
        Form::Ref { arg, binds, .. } => std::iter::once(arg.as_ref())
            .chain(binds.iter().map(value))
            .collect(),
        Form::SelfRef { arg, .. } => vec![arg.as_ref()],
    }
}

/// Every written node lies inside its parent's bytes, and nothing reaches past the text.
fn nested(o: &Outline, src: &str) {
    assert!(o.span.start <= o.span.end && o.span.end <= src.len());
    for child in children(o) {
        if child.written {
            assert!(
                o.span.start <= child.span.start && child.span.end <= o.span.end,
                "`{}` lies inside `{}`",
                text(src, child.span),
                text(src, o.span)
            );
        }
        nested(child, src);
    }
}

const WRITTEN: [&str; 9] = [
    "0.0014822 * chaigne_askenfelt(f0, vel=vel, b=max(1.4e-4, 4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12)",
    "crop(@sva25(t, f0=261.6256, vel=4.5433) + @sva25(t, f0=329.6276, vel=4.5433), 0s, 6s)",
    "-t + -3db * (1 - exp(-t/0.25))",
    "@kick.comb(delay=0.03)",
    "lowpass(sample(@note), cutoff=700 + 60*sin(2*pi*3*t), q=0.7)",
    "self(t - 1sp)*0.5 + @kick",
    "sum(k, 1, inf, sin(2*pi*110*k*t)/k)",
    "@voice(t - 0.25b)",
    "+(2)",
];

#[test]
fn an_outline_is_the_tree_the_parser_builds() {
    for src in WRITTEN {
        let tree = outline(src).unwrap_or_else(|d| panic!("{src}: {d:?}"));
        assert_eq!(expr_of(&tree), parse_expr(src).expect("it parses"), "{src}");
        nested(&tree, src);
    }
}

#[test]
fn each_span_names_the_bytes_a_node_was_written_in() {
    let src = WRITTEN[0];
    let tree = outline(src).expect("it parses");
    assert_eq!(text(src, tree.span), src, "the root spans the whole text");
    let Form::Operator { at, right, .. } = &tree.form else {
        panic!("a product: {tree:?}")
    };
    assert_eq!(text(src, *at), "*");
    let Form::Call { name, at, args } = &right.form else {
        panic!("a call: {right:?}")
    };
    assert_eq!(
        (name.as_str(), text(src, *at)),
        ("chaigne_askenfelt", "chaigne_askenfelt")
    );
    assert_eq!(text(src, right.span), &src[12..]);
    let Argument::Named { name, at, value } = &args[2] else {
        panic!("b is named: {args:?}")
    };
    assert_eq!((name.as_str(), text(src, *at)), ("b", "b"));
    assert_eq!(
        text(src, value.span),
        "max(1.4e-4, 4.1e-4*pow(f0/262, 1.9))"
    );
    let Argument::Named { value, .. } = &args[3] else {
        panic!("strike_pos is named: {args:?}")
    };
    assert_eq!(text(src, value.span), "0.12");
}

/// A prefix minus is `0 - x` and a bare ref reads at `t`: neither was written, so neither
/// claims bytes of its own.
#[test]
fn the_nodes_the_parser_supplies_say_so() {
    let src = "-t + @kick";
    let tree = outline(src).expect("it parses");
    let Form::Operator { left, right, .. } = &tree.form else {
        panic!("a sum: {tree:?}")
    };
    let Form::Operator { left: zero, .. } = &left.form else {
        panic!("0 - t: {left:?}")
    };
    assert!(!zero.written);
    assert!(left.written && text(src, left.span) == "-t");
    let Form::Ref { arg, at, .. } = &right.form else {
        panic!("a ref: {right:?}")
    };
    assert_eq!(text(src, *at), "@kick");
    assert!(!arg.written);
}

#[test]
fn a_text_that_does_not_parse_refuses_as_the_parser_does() {
    for src in ["sin(", "1 +", "2 ^ 3", ""] {
        assert_eq!(outline(src).err(), parse_expr(src).err(), "{src}");
    }
}
