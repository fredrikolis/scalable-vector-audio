// Concern: proves a render counts its operations off the schedule and refuses past the budget | Non-concern: what a row computes (sva-samples) | IO: (a composition) -> a cost tree or a refusal

use crate::fixtures::graph_of;
use sva_engine::{Ask, Output, Render, RenderConfig, Representation, Source, Tier, answer, render};

const RATE: u32 = 8_192;

fn config(secs: f64, budget: Option<u128>) -> RenderConfig {
    let mut held = RenderConfig::seconds(RATE, secs);
    if let Some(budget) = budget {
        held.flop_budget = Some(budget);
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
    render(&g, "node", config, &Tier::default()).unwrap_or_else(|e| panic!("{name}: {e}"))
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

/// FORMAT 9.2, and a real sine is two lines, each summed over the one period the second folds
/// into: 440 Hz repeats every 1024 samples at 8192 Hz.
#[test]
fn flops_of_a_line_spectrum_is_lines_times_its_period() {
    let held = rendered(
        "flops-lines",
        &[("node", "sin(2*pi*440*t)\n")],
        config(1.0, None),
    );
    let tree = counted(&held);
    assert_eq!(tree.total, 2 * 1024);
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
    let (code, text) = match render(&g, "node", config(1.0, Some(1_000)), &Tier::default()) {
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
        ("node", "sample(@heavy(t)) + 0.5*self[idx(t) - 1]\n"),
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
    let code = match render(&g, "node", config(1.0, Some(500)), &Tier::default()) {
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
        render(&g, "node", config(1.0, Some(budget)), &Tier::default()).is_err(),
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

/// A filter over a form is the form it composes to, its lines each read through the response,
/// and naming what dominates is the whole of what the tree is for.
#[test]
fn flops_tree_names_the_law_read_through_a_filter() {
    let files = &[
        (
            "node",
            "sample(lowpass(@heavy(t), 800, 0.7)) + 0.5*self[idx(t) - 1]\n",
        ),
        ("heavy", "sum(k, 1, 300, (1/k)*sin(2*pi*30*k*t))\n"),
    ];
    let held = rendered("flops-filtered-read", files, counting(1.0, Some(500)));
    let tree = counted(&held);
    let law = tree
        .rows
        .iter()
        .find(|r| r.depth == 1 && r.route.starts_with("line spectrum"))
        .unwrap_or_else(|| panic!("the filtered law is a row of its own: {:?}", tree.rows));
    assert!(law.subtree > 500, "the law it reads is what costs");

    let g = graph_of("flops-filtered-render", files);
    let text = match render(&g, "node", config(1.0, Some(500)), &Tier::default()) {
        Err(refusal) => refusal.to_string(),
        Ok(_) => panic!("the referenced law's cost is the render's cost too"),
    };
    assert!(
        text.contains("line spectrum"),
        "the refusal names it too: {text}"
    );
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

/// Two refs reaching one value pay for it once: the first row names it, and the second reads it
/// as a shared row of no cost. Charging each the whole of it read as costing more than the
/// render pays, row by row.
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
    let again = children_of(
        &tree.rows,
        tree.rows.iter().position(|r| *r == second).expect("b"),
    )
    .find(|r| r.node == "shared")
    .unwrap_or_else(|| panic!("`b` reads the shared value too: {:?}", tree.rows));
    assert!(
        again.shared && again.subtree == 0,
        "the ref that reached the value second says so: {again:?}"
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

/// A sum visits each cropped term only inside its window, so each is priced over its own
/// support, not over the whole sum's.
#[test]
fn a_cropped_term_is_priced_over_its_own_support() {
    let term = "tanh(3*sin(2*pi*55*t))";
    let price = |name: &str, text: String, secs: f64| {
        let tree = counted(&rendered(name, &[("node", &text)], counting(secs, None)));
        assert_eq!(tree.rows[0].route, "point sampling", "{name}");
        tree.total
    };
    let alone = price("flops-crop-alone", format!("crop({term}, 0s, 1s)\n"), 1.0);
    let apart = price(
        "flops-crop-apart",
        format!("crop({term}, 0s, 1s) + crop({term}, 3s, 4s)\n"),
        4.0,
    );
    let rate = u128::from(RATE);
    assert_eq!(
        apart,
        4 * rate + 2 * alone,
        "an add at each of 4 s of instants, and each term only over the second its crop holds"
    );
}

/// With no budget of its own, a render pays the one its profile names.
#[test]
fn a_render_with_no_budget_of_its_own_pays_its_profiles() {
    let g = graph_of(
        "flops-profile-budget",
        &[("node", "sum(k, 1, 200, (1/k)*sin(2*pi*100*k*t))\n")],
    );
    let held = RenderConfig {
        profile: sva_engine::Profile {
            flop_budget: 1_000,
            ..sva_engine::PSYCHOACOUSTIC_V1
        },
        ..RenderConfig::seconds(RATE, 1.0)
    };
    let Err(refused) = render(&g, "node", held, &Tier::default()) else {
        panic!("a render past its profile's budget refuses");
    };
    assert_eq!(refused.code(), "collapse.over_budget", "{refused}");
}

/// The count a reading prints, the work a render did and the price its budget weighed are one
/// number, whatever route each value takes: a transform pays per frame it transforms, never
/// one per sample.
#[test]
fn the_count_is_what_computing_paid_on_every_route() {
    let routes = [
        (
            "rows",
            "sin(2*pi*440*t)\n",
            "line spectrum, summed directly",
        ),
        (
            "program",
            "lowpass(sample(sin(2*pi*440*t)), cutoff=300)\n",
            "sampled program",
        ),
        (
            "istft",
            "istft(stft(sample(crop(sin(2*pi*440*t), 0s, 0.1s)), window=256sp, hop=64sp))\n",
            "inverse short-time transform",
        ),
    ];
    for (name, body, route) in routes {
        let held = rendered(
            &format!("paid-{name}"),
            &[("node", body)],
            config(0.1, None),
        );
        let tree = counted(&held);
        assert!(
            tree.rows.iter().any(|row| row.route == route),
            "{name}: {:?}",
            tree.rows
        );
        assert_eq!(tree.total, held.work().priced_flops, "{name}");
    }
    let held = rendered(
        "paid-transform",
        &[(
            "node",
            "istft(stft(sample(crop(sin(2*pi*440*t), 0s, 0.1s)), window=256sp, hop=64sp))\n",
        )],
        config(0.1, None),
    );
    let tree = counted(&held);
    let inverse = tree
        .rows
        .iter()
        .find(|row| row.route == "inverse short-time transform")
        .expect("the inverse");
    let samples = (0.1 * f64::from(RATE)).ceil() as usize;
    let frames = (samples + 2 * (256 - 64)).div_ceil(64) as u128;
    assert_eq!(
        inverse.own,
        frames * 256 * 8,
        "256 * log2(256) butterflies a frame"
    );
}
