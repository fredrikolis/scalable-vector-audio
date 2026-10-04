// Concern: proves a `sum` whose index a node's binding reads is its terms written out, or refuses | Non-concern: a sum no binding reads (sva-engine) | IO: (node text) -> Expr or a Diag

use sva_ast::{DiagCode, parse_expr, parse_file, render_expr};

fn body(text: &str) -> Result<String, DiagCode> {
    parse_file("bank", text)
        .map(|p| render_expr(&p.expr))
        .map_err(|d| d.code)
}

fn printed(text: &str) -> String {
    render_expr(&parse_expr(text).expect("an expression"))
}

#[test]
fn an_index_a_binding_reads_writes_the_sum_out_one_ref_per_index() {
    assert_eq!(
        body("sum(k, 1, 3, @band(t, fc=100*k)/k)\n"),
        Ok(printed(
            "@band(t, fc=100*1)/1 + @band(t, fc=100*2)/2 + @band(t, fc=100*3)/3"
        ))
    );
    assert_eq!(
        body("sum(k, 2 - 1, 2, band(t, fc=k))\n"),
        Ok(printed("band(t, fc=1) + band(t, fc=2)")),
        "a node called by name binds too, and a bound may fold from literals"
    );
}

#[test]
fn a_sum_no_binding_reads_stays_a_series() {
    for text in [
        "sum(k, 1, inf, sin(2*pi*k*220*t)/k)",
        "sum(k, 1, 3, k*@band(t, fc=100))",
        "sum(k, 1, 3, @band(t, fc=sum(k, 1, 2, k)))",
    ] {
        assert_eq!(body(&format!("{text}\n")), Ok(printed(text)), "{text}");
    }
}

#[test]
fn an_inner_sum_rebinding_the_index_keeps_its_own() {
    assert_eq!(
        body("sum(k, 1, 2, @band(t, fc=k) + sum(k, 1, k, k))\n"),
        Ok(printed(
            "@band(t, fc=1) + sum(k, 1, 1, k) + (@band(t, fc=2) + sum(k, 1, 2, k))"
        ))
    );
}

#[test]
fn an_outer_index_bounding_an_inner_written_sum_writes_both_out() {
    assert_eq!(
        body("sum(k, 1, 2, sum(j, 1, k, @band(t, fc=j)))\n"),
        Ok(printed(
            "@band(t, fc=1) + (@band(t, fc=1) + @band(t, fc=2))"
        ))
    );
}

/// A bound naming a parameter is a number each instance knows, so the sum waits for one.
#[test]
fn a_sum_bounded_by_a_parameter_waits_for_its_instance() {
    let text = "sum(k, 1, n, @band(t, fc=k))";
    assert_eq!(body(&format!("{text}\n")), Ok(printed(text)));
}

#[test]
fn a_sum_with_no_written_count_of_nodes_refuses() {
    for text in [
        "sum(k, 1, inf, @band(t, fc=k))",
        "sum(k, 1, n % 2, @band(t, fc=k))",
        "sum(k, 1, 100000, @band(t, fc=k))",
        "sum(a, 1, 4000, sum(b, 1, 4000, @band(t, fc=a + b)))",
    ] {
        assert_eq!(
            body(&format!("{text}\n")),
            Err(DiagCode::UnwrittenSeries),
            "{text}"
        );
    }
}
