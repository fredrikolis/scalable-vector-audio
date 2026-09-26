// Concern: proves a render counts its operations off the schedule and refuses past the budget | Non-concern: what a row computes (sva-samples) | IO: (a composition) -> a cost tree or a refusal

mod fixtures;

use fixtures::graph_of;
use sva_engine::{Ask, Output, Render, RenderConfig, Representation, Source, answer, render};

const RATE: u32 = 8_192;

fn config(secs: f64, budget: Option<u128>) -> RenderConfig {
    let mut held = RenderConfig::seconds(RATE, secs);
    if let Some(budget) = budget {
        held.flop_budget = budget;
    }
    held
}

/// A render that only counts materializes nothing, whatever the count comes to.
fn counting(secs: f64, budget: Option<u128>) -> RenderConfig {
    config(secs, budget).asking(vec![Ask {
        node: "node".to_string(),
        representation: Representation::Flops,
    }])
}

fn rendered(name: &str, files: &[(&str, &str)], config: RenderConfig) -> Render {
    let g = graph_of(name, files);
    render(&g, "node", config, None).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn counted(render: &Render) -> sva_engine::FlopTree {
    let id = render.id("node").expect("the root");
    match answer(render, id, Representation::Flops).expect("a count") {
        sva_engine::Answer {
            value: Output::Flops(tree),
            source: Source::Exact,
            ..
        } => *tree,
        other => panic!("a count is exact arithmetic, not {other:?}"),
    }
}

/// FORMAT 9.2, and a real sine is two lines.
#[test]
fn flops_of_a_line_spectrum_is_lines_times_samples() {
    let held = rendered(
        "flops-lines",
        &[("node", "sin(2*pi*440*t)\n")],
        config(1.0, None),
    );
    let tree = counted(&held);
    assert_eq!(tree.total, 2 * u128::from(RATE));
    let root = tree.rows.first().expect("the root row");
    assert_eq!(root.node, "node");
    assert_eq!(root.own, tree.total);
    assert_eq!(root.route, "line spectrum, summed directly");
    assert!(
        (root.percent - 100.0).abs() < 1e-9,
        "the root is the whole of it"
    );
}

/// A count is not a render: asking what a reading costs never pays for it.
#[test]
fn a_render_over_budget_refuses_naming_the_dominating_node() {
    let files = &[
        ("node", "@loud(t) + @quiet(t)\n"),
        ("loud", "sum(k, 1, 400, (1/k)*sin(2*pi*100*k*t))\n"),
        ("quiet", "sin(2*pi*55*t)\n"),
    ];
    let held = rendered("flops-budget-count", files, counting(1.0, Some(1_000)));
    let tree = counted(&held);
    assert!(tree.total > 1_000, "the count itself is never refused");

    let g = graph_of("flops-budget-render", files);
    let (code, text) = match render(&g, "node", config(1.0, Some(1_000)), None) {
        Err(refusal) => (refusal.code().to_string(), refusal.to_string()),
        Ok(_) => panic!("a render past its budget refuses"),
    };
    assert_eq!(code, "collapse.over_budget", "{text}");
    assert!(
        text.contains("loud"),
        "the dominating node is named: {text}"
    );
    assert!(text.contains("line spectrum"), "its route is named: {text}");
    assert!(
        text.contains(&tree.total.to_string()),
        "the count {} to pass is named: {text}",
        tree.total
    );
}

/// A sampled node is its own program over buffers beside it, so the form it reads is a row of
/// its own.
#[test]
fn a_sampled_node_counts_the_law_it_reads_beside_its_own_program() {
    let files = &[
        ("node", "sample(@heavy(t)) + 0.5*self(t - 1sp)\n"),
        ("heavy", "sum(k, 1, 300, (1/k)*sin(2*pi*30*k*t))\n"),
    ];
    let held = rendered("flops-sampled-count", files, counting(1.0, Some(500)));
    let tree = counted(&held);
    let law = tree
        .rows
        .iter()
        .find(|r| r.node == "heavy")
        .expect("the law it reads is a row of its own");
    assert!(law.subtree > 500, "the law it reads is what costs");
    assert!(
        tree.total >= law.subtree,
        "a buffer beside the program adds to it: {} against {}",
        tree.total,
        law.subtree
    );

    let g = graph_of("flops-sampled-render", files);
    let code = match render(&g, "node", config(1.0, Some(500)), None) {
        Err(refusal) => refusal.code().to_string(),
        Ok(_) => panic!("the referenced law's cost is the render's cost too"),
    };
    assert_eq!(code, "collapse.over_budget");
}

#[test]
fn the_flag_admits_the_cost_and_the_label_carries_it() {
    let files = &[("node", "sum(k, 1, 200, (1/k)*sin(2*pi*100*k*t))\n")];
    let budget = 1_000;
    let g = graph_of("flops-admitted", files);
    assert!(
        render(&g, "node", config(1.0, Some(budget)), None).is_err(),
        "under budget it refuses"
    );

    let admitted = rendered("flops-admitted", files, config(1.0, Some(u128::MAX)));
    let label = admitted
        .labels
        .get(&admitted.root)
        .expect("the root collapsed");
    let cost = label.cost.expect("every render label carries its count");
    assert_eq!(cost.budget, u128::MAX);
    assert_eq!(cost.flops, counted(&admitted).total);
    assert!(cost.flops > budget);
}

/// A filter between a sampled root and the form it reads is still a read, and naming what
/// dominates is the whole of what the tree is for.
#[test]
fn flops_tree_names_the_law_read_through_a_filter() {
    let files = &[
        (
            "node",
            "sample(lowpass(@heavy(t), 800, 0.7)) + 0.5*self(t - 1sp)\n",
        ),
        ("heavy", "sum(k, 1, 300, (1/k)*sin(2*pi*30*k*t))\n"),
    ];
    let held = rendered("flops-filtered-read", files, counting(1.0, Some(500)));
    let tree = counted(&held);
    let law = tree
        .rows
        .iter()
        .find(|r| r.node == "heavy")
        .unwrap_or_else(|| panic!("the law read through the filter is named: {:?}", tree.rows));
    assert!(law.subtree > 500, "the law it reads is what costs");

    let g = graph_of("flops-filtered-render", files);
    let text = match render(&g, "node", config(1.0, Some(500)), None) {
        Err(refusal) => refusal.to_string(),
        Ok(_) => panic!("the referenced law's cost is the render's cost too"),
    };
    assert!(text.contains("heavy"), "the refusal names it too: {text}");
}

fn children_of(
    rows: &[sva_engine::FlopRow],
    at: usize,
) -> impl Iterator<Item = &sva_engine::FlopRow> {
    let depth = rows[at].depth;
    rows[at + 1..]
        .iter()
        .take_while(move |r| r.depth > depth)
        .filter(move |r| r.depth == depth + 1)
}

/// Two refs reaching one evaluation tree pay for it once: the first row names it, and the second
/// is priced net of it. Charging each the whole of it read as costing more than the render pays,
/// row by row.
#[test]
fn sibling_rows_never_sum_past_their_parent() {
    let heavy = "sum(k, 1, 200, (1/k)*sin(2*pi*100*k*t))\n";
    let glide = "sin(2*pi*100*pow(2, t/4)*t)\n";
    let files = &[
        ("node", "@a(t) + @b(t)\n"),
        ("a", "@shared(t)*2\n"),
        ("b", "@shared(t) * @other(t)\n"),
        ("shared", heavy),
        ("other", glide),
    ];
    let tree = counted(&rendered("flops-shared", files, counting(1.0, None)));
    for (at, row) in tree.rows.iter().enumerate() {
        let under: u128 = children_of(&tree.rows, at).map(|c| c.subtree).sum();
        assert!(
            under <= row.subtree,
            "`{}` is made of its rows, which come to {under} against its own {}: {:?}",
            row.node,
            row.subtree,
            tree.rows
        );
    }

    let named = |node: &str| {
        tree.rows
            .iter()
            .find(|r| r.node == node)
            .unwrap_or_else(|| panic!("a row for `{node}`: {:?}", tree.rows))
            .clone()
    };
    let (held, second) = (named("shared"), named("b"));
    assert!(
        second.shared,
        "the ref that reached the tree second says so: {second:?}"
    );

    // The same product with nothing else reading the heavy tree: what `b` costs on its own.
    let apart = counted(&rendered(
        "flops-apart",
        &[
            ("node", "@shared(t) * @other(t)\n"),
            ("shared", heavy),
            ("other", glide),
        ],
        counting(1.0, None),
    ));
    assert_eq!(
        second.subtree + held.subtree,
        apart.total,
        "the two rows come to the one evaluation, with the tree under exactly one of them: \
         {second:?} beside {held:?}"
    );
}

/// Priced at one operation a sample, a pointwise row summed to less than the rows under it —
/// a tree two of its refs reach is charged under one of them, not both.
#[test]
fn a_pointwise_root_prices_at_least_its_children() {
    let bass = "lowpass(sin(2*pi*110*t), cutoff=400, q=3) * 0.3\n";
    let (left, right) = ("@bass(t)*0.5 + tanh(t)\n", "@bass(t)*0.3 + tanh(2*t)\n");
    let tree = counted(&rendered(
        "flops-pointwise",
        &[
            ("node", "@a(t) + @b(t)\n"),
            ("a", left),
            ("b", right),
            ("bass", bass),
        ],
        counting(1.0, None),
    ));
    let root = tree.rows.first().expect("the root row").clone();
    assert_eq!(root.route, "point sampling");
    assert_eq!(
        root.own,
        3 * u128::from(RATE),
        "`@a(t) + @b(t)` walks an add over two reads at every instant"
    );

    let alone = |name: &str, text: &str| {
        counted(&rendered(
            name,
            &[("node", text), ("bass", bass)],
            counting(1.0, None),
        ))
        .total
    };
    let held = alone("flops-pointwise-bass", bass);
    assert_eq!(
        tree.total,
        root.own + alone("flops-pointwise-a", left) + alone("flops-pointwise-b", right) - held,
        "the two refs and the one tree both of them reach, that tree charged once"
    );

    let under: u128 = children_of(&tree.rows, 0).map(|c| c.subtree).sum();
    assert!(
        under <= root.subtree,
        "the rows under a pointwise root came to {under} against its own {}: {:?}",
        root.subtree,
        tree.rows
    );
}
