// Concern: writes an expression's outline as the `data` object both front ends answer | Non-concern: building the outline (sva-ast) | IO: (&str) -> a JSON string or CliError

use sva_ast::outline::{Argument, Form};
use sva_ast::{BinOp, ByteSpan, Literal, Outline, Refusal};

use crate::CliError;
use crate::json::{escape, list, num};

/// `{ "text", "outline" }`, or the parser's own refusal, located in the text.
pub fn outline_data(text: &str) -> Result<String, CliError> {
    let tree = sva_ast::outline(text).map_err(|d| {
        CliError::Refusals(vec![Refusal::new("expression", d.span, d.code, d.message)])
    })?;
    Ok(format!(
        "{{ \"text\": \"{}\", \"outline\": {} }}",
        escape(text),
        node(&tree)
    ))
}

fn span(at: ByteSpan) -> String {
    format!("{{ \"start\": {}, \"end\": {} }}", at.start, at.end)
}

fn node(o: &Outline) -> String {
    let (kind, fields) = match &o.form {
        Form::Literal(l) => ("literal", literal(l)),
        Form::Name(name) => ("name", format!("\"name\": \"{}\"", escape(name))),
        Form::Operator {
            op,
            at,
            left,
            right,
        } => (
            "operator",
            format!(
                "\"op\": \"{}\", \"at\": {}, \"left\": {}, \"right\": {}",
                symbol(*op),
                span(*at),
                node(left),
                node(right)
            ),
        ),
        Form::Call { name, at, args } => (
            "call",
            format!(
                "\"name\": \"{}\", \"at\": {}, \"args\": {}",
                escape(name),
                span(*at),
                list(args, argument)
            ),
        ),
        Form::Ref {
            path,
            at,
            arg,
            binds,
        } => (
            "ref",
            format!(
                "\"path\": \"{}\", \"at\": {}, \"arg\": {}, \"binds\": {}",
                escape(path),
                span(*at),
                node(arg),
                list(binds, argument)
            ),
        ),
        Form::SelfRef { at, arg } => (
            "self",
            format!("\"at\": {}, \"arg\": {}", span(*at), node(arg)),
        ),
    };
    format!(
        "{{ \"kind\": \"{kind}\", \"span\": {}, \"written\": {}, {fields} }}",
        span(o.span),
        o.written
    )
}

fn argument(a: &Argument) -> String {
    match a {
        Argument::Positional(value) => {
            format!("{{ \"kind\": \"positional\", \"value\": {} }}", node(value))
        }
        Argument::Named { name, at, value } => format!(
            "{{ \"kind\": \"named\", \"name\": \"{}\", \"at\": {}, \"value\": {} }}",
            escape(name),
            span(*at),
            node(value)
        ),
    }
}

/// A unit the lexer resolves, `s` or `db`, is a number already; bars and samples wait for a
/// tempo and a rate.
fn literal(l: &Literal) -> String {
    let (unit, value) = match l {
        Literal::Num(v) => ("number", num(*v)),
        Literal::Bars(v) => ("bars", num(*v)),
        Literal::Samples(v) => ("samples", num(*v)),
        Literal::Str(s) => ("text", format!("\"{}\"", escape(s))),
    };
    format!("\"unit\": \"{unit}\", \"value\": {value}")
}

fn symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
    }
}
