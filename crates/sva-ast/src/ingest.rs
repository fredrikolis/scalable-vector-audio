// Concern: classifies one file's raw content and parses it to an Expr | Non-concern: walking the directory, resolving refs (graph.rs) | IO: (name, content) -> Expr or a Diag

use crate::diag::{ByteSpan, Diag, DiagCode};
use crate::expr::{Arg, Binds, Expr, Literal, children};
use crate::filename::{SpanUnit, parse_filename};
use crate::parser::parse;
use crate::tsv::{Grid, materialize, parse_grid};

/// A grid file yields both — the expression every other pass reads, and the cells it came
/// from, so a later tempo resolution can re-derive the expression at the right unit.
#[derive(Debug)]
pub struct Parsed {
    pub expr: Expr,
    pub grid: Option<Grid>,
    pub defaults: Vec<(String, Expr)>,
}

/// A leading `<name> = <expr>` line states what a caller that says nothing about `<name>` gets.
/// Everything after the last one is the body, read exactly as a file with no defaults is.
pub fn parse_file(base_name: &str, content: &str) -> Result<Parsed, Diag> {
    let rows = crate::tsv::code_rows(content);
    let split = rows
        .iter()
        .position(|(_, line)| !is_default_line(line))
        .unwrap_or(rows.len());
    let (heads, _) = rows.split_at(split);
    let body_at = rows.get(split).map_or(content.len(), |(at, _)| *at);
    let defaults = read_defaults(heads)?;

    let mut parsed = parse_body(base_name, &content[body_at..]).map_err(|d| shift(d, body_at))?;
    // Liveness runs back from the body: only a live default's own reads count.
    let mut live = vec![false; defaults.len()];
    for at in (0..defaults.len()).rev() {
        let name = &defaults[at].0;
        live[at] = mentions(&parsed.expr, name)
            || defaults[at + 1..]
                .iter()
                .enumerate()
                .any(|(k, (_, value))| live[at + 1 + k] && mentions(value, name));
    }
    for (at, (name, _)) in defaults.iter().enumerate() {
        if !live[at] {
            return Err(Diag::new(
                DiagCode::BadDefault,
                ByteSpan::new(0, body_at),
                format!("`{name}` has a default but the body never reads it"),
            ));
        }
    }
    parsed.defaults = defaults;
    Ok(parsed)
}

/// A grid row never opens with one: a cell is an expression, never an assignment.
fn is_default_line(line: &str) -> bool {
    let code = line.trim_start();
    let end = code
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(code.len());
    let (name, rest) = code.split_at(end);
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && rest.trim_start().starts_with('=')
}

fn read_defaults(heads: &[(usize, &str)]) -> Result<Vec<(String, Expr)>, Diag> {
    let mut out: Vec<(String, Expr)> = Vec::new();
    for (at, line) in heads {
        let eq = line.find('=').expect("is_default_line found one");
        let name = line[..eq].trim().to_string();
        let value = parse(&line[eq + 1..])
            .map(|e| relocate(&e, at + eq + 1))
            .map_err(|d| shift(d, at + eq + 1))?;
        if out.iter().any(|(seen, _)| *seen == name) {
            return Err(Diag::new(
                DiagCode::BadDefault,
                ByteSpan::new(*at, at + line.len()),
                format!("`{name}` is given a default twice"),
            ));
        }
        out.push((name, value));
    }
    Ok(out)
}

/// A default is parsed out of its own line, so every span it carries has to be put back where
/// the file reads — a refusal inside one is located like any other.
fn relocate(e: &Expr, by: usize) -> Expr {
    let moved = |s: &ByteSpan| ByteSpan::new(s.start + by, s.end + by);
    match e {
        Expr::Lit(_) | Expr::Var(_) => e.clone(),
        Expr::Bin(op, l, r) => Expr::Bin(*op, Box::new(relocate(l, by)), Box::new(relocate(r, by))),
        Expr::SelfRef { arg, span } => Expr::SelfRef {
            arg: Box::new(relocate(arg, by)),
            span: moved(span),
        },
        Expr::Ref {
            path,
            arg,
            binds,
            span,
        } => Expr::Ref {
            path: path.clone(),
            arg: Box::new(relocate(arg, by)),
            binds: binds
                .iter()
                .map(|(k, v)| (k.clone(), relocate(v, by)))
                .collect(),
            span: moved(span),
        },
        Expr::Call { name, args, span } => Expr::Call {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| match a {
                    Arg::Pos(e) => Arg::Pos(relocate(e, by)),
                    Arg::Named(k, e) => Arg::Named(k.clone(), relocate(e, by)),
                })
                .collect(),
            span: moved(span),
        },
    }
}

fn shift(d: Diag, by: usize) -> Diag {
    Diag::new(
        d.code,
        ByteSpan::new(d.span.start + by, d.span.end + by),
        d.message,
    )
}

/// Syntactic: a name the body never writes down, in any position, has a dead default.
pub fn mentions(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Var(n) => n == name,
        Expr::Call { name: called, .. } if called == name => true,
        _ => children(e, Binds::Substitute)
            .into_iter()
            .any(|c| mentions(c, name)),
    }
}

/// A single-line file whose content is neither a plain number nor a bare token is parsed as
/// a full expression; anything with a tab or more than one non-trivial line is a TSV grid.
fn parse_body(base_name: &str, content: &str) -> Result<Parsed, Diag> {
    if let Some(why) = not_node_marker(content) {
        return Err(not_node_diag(why));
    }
    let (_, span) = parse_filename(base_name);
    let rows = crate::tsv::code_rows(content);
    let is_tsv = rows.iter().any(|(_, line)| line.contains('\t')) || rows.len() > 1;

    if is_tsv {
        let Some(span) = span else {
            return Err(missing_span_diag(content.len()));
        };
        let mut grid = parse_grid(content)?;
        grid.bar_span = matches!(span.unit, SpanUnit::Bars).then_some(span.amount);
        let expr = materialize(&grid, span.amount);
        return Ok(Parsed {
            expr,
            grid: Some(grid),
            defaults: Vec::new(),
        });
    }

    let expr = parse_scalar_file(content, &rows)?;
    Ok(Parsed {
        expr,
        grid: None,
        defaults: Vec::new(),
    })
}

/// Classified from the comment-stripped row, never the raw text, so a `; ...` header cannot
/// leave `4/4` looking like a division. The expression arm still parses the whole file.
fn parse_scalar_file(content: &str, rows: &[(usize, &str)]) -> Result<Expr, Diag> {
    let code = rows.first().map_or("", |(_, line)| *line).trim();
    if code.is_empty() {
        return Ok(Expr::Lit(Literal::Str(String::new())));
    }
    if let Ok(n) = code.parse::<f64>()
        && n.is_finite()
    {
        return Ok(Expr::Lit(Literal::Num(n)));
    }
    if is_bare_token(code) {
        return Ok(Expr::Lit(Literal::Str(code.to_string())));
    }
    parse(content)
}

/// The one whole-file value the grammar cannot spell is a meter like `4/4`, whose `/` would
/// otherwise divide. Everything else is math: `C4` is a note, `0.25b` a bar literal.
fn is_bare_token(s: &str) -> bool {
    s.contains('/')
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '/' | '_'))
}

/// Narrow, cheap: catches bench's own generated HTML landing in a composition directory,
/// which `rows.len() > 1` would otherwise misclassify as a TSV grid.
fn not_node_marker(content: &str) -> Option<&'static str> {
    let mut end = content.len().min(2048);
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    let head = content[..end].to_ascii_lowercase();
    if head.contains("<!doctype html") {
        Some("starts with `<!DOCTYPE html>`")
    } else if head.contains("<html") {
        Some("contains an `<html>` tag")
    } else {
        None
    }
}

fn not_node_diag(why: &str) -> Diag {
    use crate::diag::{ByteSpan, DiagCode};
    Diag::new(
        DiagCode::NotNodeContent,
        ByteSpan::at(0),
        format!("this file does not parse as a node: it {why}, which looks like HTML"),
    )
}

fn missing_span_diag(len: usize) -> Diag {
    use crate::diag::{ByteSpan, DiagCode};
    Diag::new(
        DiagCode::TsvMissingSpan,
        ByteSpan::at(len),
        "a TSV step grid needs a filename span suffix (<name>-<n>b or <name>-<n>s) to know row duration",
    )
}

pub fn base_name(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_number_files_are_literals() {
        let p = parse_file("bpm", "120\n").unwrap();
        assert_eq!(p.expr, Expr::Lit(Literal::Num(120.0)));
        assert!(p.grid.is_none());
    }

    #[test]
    fn bare_token_files_are_string_literals_not_expressions() {
        assert_eq!(
            parse_file("meter", "4/4\n").unwrap().expr,
            Expr::Lit(Literal::Str("4/4".to_string()))
        );
    }

    #[test]
    fn a_file_holding_one_identifier_is_that_identifier_not_a_string() {
        assert_eq!(
            parse_file("root", "C4\n").unwrap().expr,
            Expr::Var("C4".to_string())
        );
        assert_eq!(
            parse_file("gain", "pi\n").unwrap().expr,
            Expr::Var("pi".to_string())
        );
    }

    #[test]
    fn single_line_expression_files_parse_as_expressions() {
        let p = parse_file("kick", "sin(2*pi*50*t) * exp(-t*30)\n").unwrap();
        assert!(matches!(p.expr, Expr::Bin(..)));
        assert!(p.grid.is_none(), "a one-line file is not a grid");
    }

    #[test]
    fn a_tsv_file_without_a_span_suffix_is_a_located_refusal() {
        let err = parse_file("pattern", "@kick\n@snare\n").unwrap_err();
        assert_eq!(err.code, crate::diag::DiagCode::TsvMissingSpan);
    }

    #[test]
    fn html_content_refuses_as_not_a_node_rather_than_a_tsv_missing_span() {
        let html = "<!DOCTYPE html>\n<html>\n<head></head>\n<body>bench output</body>\n</html>\n";
        let err = parse_file("bench", html).unwrap_err();
        assert_eq!(err.code, crate::diag::DiagCode::NotNodeContent);
        assert!(err.message.contains("HTML"));
    }

    #[test]
    fn comment_lines_count_as_neither_expression_lines_nor_grid_rows() {
        let p = parse_file("kick", "; the body\nsin(2*pi*50*t)\n").unwrap();
        assert!(p.grid.is_none());
        assert_eq!(p.expr, parse_file("kick", "sin(2*pi*50*t)\n").unwrap().expr);

        let commented = parse_file("pattern-1b", "; lane\n@kick\n@kick\n").unwrap();
        let bare = parse_file("pattern-1b", "@kick\n@kick\n").unwrap();
        assert_eq!(
            commented.expr, bare.expr,
            "a comment row would otherwise stretch every row's position"
        );
    }

    #[test]
    fn a_header_comment_leaves_every_scalar_shape_alone() {
        let header = "; Concern: the time signature | Non-concern: the tempo | IO: none\n";
        assert_eq!(
            parse_file("meter", &format!("{header}4/4\n")).unwrap().expr,
            Expr::Lit(Literal::Str("4/4".to_string()))
        );
        assert_eq!(
            parse_file("bpm", &format!("{header}120\n")).unwrap().expr,
            Expr::Lit(Literal::Num(120.0))
        );
        assert_eq!(
            parse_file("root", &format!("{header}C4\n")).unwrap().expr,
            Expr::Var("C4".to_string())
        );
        assert_eq!(
            parse_file("meter", "4/4 ; four on the floor\n")
                .unwrap()
                .expr,
            Expr::Lit(Literal::Str("4/4".to_string())),
            "a trailing comment is stripped too"
        );
        assert_eq!(
            parse_file("gain", "; only a comment\n").unwrap().expr,
            Expr::Lit(Literal::Str(String::new())),
            "a file with no code is empty, not malformed"
        );
    }

    #[test]
    fn a_tabbed_single_line_file_is_still_a_tsv_grid() {
        let p = parse_file("pattern-1b", "@kick\t@snare").unwrap();
        assert!(matches!(p.expr, Expr::Bin(..)));
        assert_eq!(p.grid.unwrap().cells.len(), 2);
    }

    /// The bar count comes off the filename itself, not off `bpm`/`meter`, and stays absent
    /// for a grid whose span is already in seconds.
    #[test]
    fn a_grids_bar_span_comes_from_its_filename_suffix() {
        let bars = parse_file("pattern-4b", "@kick\n@kick\n").unwrap();
        assert_eq!(bars.grid.unwrap().bar_span, Some(4.0));

        let secs = parse_file("pattern-2s", "@kick\n@kick\n").unwrap();
        assert_eq!(secs.grid.unwrap().bar_span, None);
    }

    #[test]
    fn malformed_expression_syntax_refuses() {
        assert!(parse_file("lead", "sin(\n").is_err());
    }
}
