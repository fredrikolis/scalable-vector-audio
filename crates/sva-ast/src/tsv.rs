// Concern: splits a TSV step grid into unit-free cells and sums them against a span | Non-concern: the cell grammar (parser.rs), the filename span suffix (filename.rs) | IO: (&str) -> Grid -> Expr

use crate::diag::{ByteSpan, Diag};
use crate::expr::{Arg, BinOp, Binds, Expr, Literal, map_children_ok};
use crate::parser::parse;

/// `cells` pairs each cell with its row's FRACTION of the file's span; `row_count`/`bar_span`
/// are lint's rows-per-bar inputs, not recoverable from `cells` alone.
#[derive(Clone, Debug)]
pub struct Grid {
    pub cells: Vec<(f64, Expr)>,
    pub row_count: usize,
    /// The filename's `-Nb` bar count, `None` for `-Ns`, captured before `resolve_bar_spans`
    /// overwrites `FileSpan::unit` to `Seconds` and erases that distinction.
    pub bar_span: Option<f64>,
}

/// A blank cell contributes nothing but its row still counts toward the fraction denominator.
pub fn materialize(grid: &Grid, span_amount: f64) -> Expr {
    grid.cells
        .iter()
        .map(|(row_fraction, cell)| {
            let row_offset = row_fraction * span_amount;
            if row_offset == 0.0 {
                cell.clone()
            } else {
                place(cell, row_offset)
            }
        })
        .reduce(|acc, t| Expr::Bin(BinOp::Add, Box::new(acc), Box::new(t)))
        .unwrap_or(Expr::Lit(Literal::Num(0.0)))
}

/// `1[a,b)(t - offset)` is `1[a+offset,b+offset)(t)`: a cell's window rides its own row.
fn place(cell: &Expr, offset: f64) -> Expr {
    match cell {
        Expr::Var(n) if n == "t" => Expr::Bin(
            BinOp::Sub,
            Box::new(Expr::Var("t".to_string())),
            Box::new(Expr::Lit(Literal::Num(offset))),
        ),
        Expr::Call { name, args, span } if name == "crop" => {
            let mut positional = 0usize;
            let args = args
                .iter()
                .map(|arg| match arg {
                    Arg::Pos(e) => {
                        positional += 1;
                        match positional {
                            2 | 3 => Arg::Pos(later(e, offset)),
                            _ => Arg::Pos(place(e, offset)),
                        }
                    }
                    Arg::Named(key, e) if key == "start" || key == "end" => {
                        Arg::Named(key.clone(), later(e, offset))
                    }
                    Arg::Named(key, e) => Arg::Named(key.clone(), place(e, offset)),
                })
                .collect();
            Expr::Call {
                name: name.clone(),
                args,
                span: *span,
            }
        }
        // Keep: a row shift moves the read, it does not mint an instance per row.
        _ => map_children_ok(cell, Binds::Keep, |child| place(child, offset)),
    }
}

/// A `b` literal is unresolved here, so only a number folds.
fn later(bound: &Expr, offset: f64) -> Expr {
    match bound {
        Expr::Lit(Literal::Num(at)) => Expr::Lit(Literal::Num(at + offset)),
        _ => Expr::Bin(
            BinOp::Add,
            Box::new(place(bound, offset)),
            Box::new(Expr::Lit(Literal::Num(offset))),
        ),
    }
}

/// A blank line is a rest and stays a row; a comment-only line is no step and is dropped, so
/// writing one above a grid cannot shift every row's position.
pub fn code_rows(content: &str) -> Vec<(usize, &str)> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last().is_some_and(|l| l.trim().is_empty()) && lines.len() > 1 {
        lines.pop();
    }
    let mut rows = Vec::new();
    let mut at = 0usize;
    for line in lines {
        let code = crate::lexer::strip_line_comment(line);
        if code.len() == line.len() || !code.trim().is_empty() {
            rows.push((at, code));
        }
        at += line.len() + 1;
    }
    rows
}

pub fn parse_grid(content: &str) -> Result<Grid, Diag> {
    let rows = code_rows(content);
    let row_count = rows.len().max(1);

    let mut cells = Vec::new();
    for (row_idx, (line_start, line)) in rows.iter().enumerate() {
        let row_fraction = (row_idx as f64) / row_count as f64;
        let mut offset = *line_start;
        for cell in line.split('\t') {
            let cell_start = offset;
            offset += cell.len() + 1;
            let trimmed = cell.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cell_expr = parse(trimmed).map_err(|d| {
                let cell_off = cell.find(trimmed).unwrap_or(0);
                Diag::new(
                    d.code,
                    ByteSpan::new(
                        cell_start + cell_off + d.span.start,
                        cell_start + cell_off + d.span.end,
                    ),
                    d.message,
                )
            })?;
            cells.push((row_fraction, cell_expr));
        }
    }
    Ok(Grid {
        cells,
        row_count,
        bar_span: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::DiagCode;

    fn desugar(content: &str, span_amount: f64) -> Expr {
        materialize(&parse_grid(content).unwrap(), span_amount)
    }

    #[test]
    fn blank_lines_and_cells_contribute_nothing_but_count_as_rows() {
        let e = desugar("@kick*0.5\n\n@snare\t@hihat\n", 1.0);
        assert_ne!(e, Expr::Lit(Literal::Num(0.0)));
    }

    #[test]
    fn a_wholly_blank_grid_desugars_to_zero() {
        assert_eq!(desugar("\n\n", 1.0), Expr::Lit(Literal::Num(0.0)));
    }

    #[test]
    fn row_offset_shifts_every_t_in_the_cell() {
        let e = desugar("@kick\n@kick\n", 2.0);
        let Expr::Bin(BinOp::Add, first, second) = e else {
            panic!("expected a sum of two terms")
        };
        assert_eq!(
            *first,
            Expr::Ref {
                path: "kick".to_string(),
                arg: Box::new(Expr::Var("t".to_string())),
                binds: Vec::new(),
                span: ByteSpan::new(0, 5),
            }
        );
        let Expr::Ref { arg, .. } = *second else {
            panic!("expected a Ref")
        };
        assert_eq!(
            *arg,
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Var("t".to_string())),
                Box::new(Expr::Lit(Literal::Num(1.0))),
            )
        );
    }

    #[test]
    fn one_parsed_grid_rescales_to_any_span_without_reparsing() {
        let grid = parse_grid("@kick\n@kick\n").unwrap();
        let two = materialize(&grid, 2.0);
        let four = materialize(&grid, 4.0);
        assert_ne!(two, four);
        assert_eq!(materialize(&grid, 4.0), four, "materializing is pure");

        let Expr::Bin(BinOp::Add, _, second) = four else {
            panic!("expected a sum of two terms")
        };
        let Expr::Ref { arg, .. } = *second else {
            panic!("expected a Ref")
        };
        assert_eq!(
            *arg,
            Expr::Bin(
                BinOp::Sub,
                Box::new(Expr::Var("t".to_string())),
                Box::new(Expr::Lit(Literal::Num(2.0))),
            ),
            "row 1 of 2 sits half a 4-second span in"
        );
    }

    #[test]
    fn a_malformed_cell_refuses_located() {
        assert_eq!(
            parse_grid("@kick\n@snare\t(\n").unwrap_err().code,
            DiagCode::UnexpectedEof
        );
    }

    #[test]
    fn row_count_counts_every_row_a_trailing_blank_included() {
        let g = parse_grid("@kick\n@kick\n@kick\n").unwrap();
        assert_eq!(g.row_count, 3);
        let g = parse_grid("@kick\n@kick\n@kick\n\n").unwrap();
        assert_eq!(g.row_count, 4, "a trailing blank row still counts");
        assert_eq!(
            g.bar_span, None,
            "parse_grid alone knows nothing of a filename span"
        );
    }
}
