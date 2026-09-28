// Concern: proves a node is its prefix form, bit for bit, before each switch the machine places, whole and streamed | Non-concern: storing runs under it | IO: (N, before-form, r) -> samples

use sva_ast::Graph;

use crate::render::{Range, RenderConfig, Stream, StreamConfig, plan, render};

const RATE: u32 = 44_100;
const SECS: f64 = 1.0;

/// A node written with a switch at `r`, and the form it has before that switch; a solver
/// is dear, so it is held to fewer instants and blocks.
struct Case {
    name: &'static str,
    switched: fn(f64) -> String,
    before: &'static str,
    solver: bool,
}

const MACHINE: &[Case] = &[
    Case {
        name: "a sampled crop falling into r",
        switched: |r| format!("crop(sample(@tone), 0s, {r}s, fall=0.02s)\n"),
        before: "crop(sample(@tone), 0s, inf)\n",
        solver: false,
    },
    Case {
        name: "a sampled crop rising from r",
        switched: |r| format!("sample(@tone) + crop(sample(@tone), {r}s, 3600s, rise=0.01s)\n"),
        before: "sample(@tone) + 0\n",
        solver: false,
    },
    Case {
        name: "a filter over samples handed over at r",
        switched: |r| {
            format!(
                "lowpass(crop(sample(@tone), 0s, {r}s) + crop(sample(0.5*@tone), {r}s, 3600s), \
                 cutoff=900, q=0.7)\n"
            )
        },
        before: "lowpass(crop(sample(@tone), 0s, inf) + 0, cutoff=900, q=0.7)\n",
        solver: false,
    },
    Case {
        name: "a loop over a sampled crop ending at r",
        switched: |r| format!("crop(sample(@tone), 0s, {r}s) + 0.4*self(t - 0.05s)\n"),
        before: "crop(sample(@tone), 0s, inf) + 0.4*self(t - 0.05s)\n",
        solver: false,
    },
    Case {
        name: "a bow pressed harder from r",
        switched: |r| {
            format!("willemsen_bilbao_serafin(196, bow_force=2 + crop(2, {r}s, 3600s))\n")
        },
        before: "willemsen_bilbao_serafin(196, bow_force=2)\n",
        solver: true,
    },
    Case {
        name: "a felt damping from r",
        switched: |r| {
            format!("chaigne_askenfelt(261.63, release=0, damper_r=0.1*crop(1, {r}s, 3600s))\n")
        },
        before: "chaigne_askenfelt(261.63, release=0, damper_r=0)\n",
        solver: true,
    },
];

/// Rows sum these in an order their prefix forms do not share, a last bit apart before the
/// switch, so none of their switches is placed.
const ROWS: &[fn(f64) -> String] = &[
    |r| format!("crop(@tone, 0s, {r}s) + crop(0.5*@tone*exp(-(t - {r})/0.2), {r}s, 3600s)\n"),
    |r| format!("crop(@tone, 0s, {r}s, fall=0.05s)\n"),
    |r| format!("@tone + crop(@tone, {r}s, 3600s, rise=0.01s)\n"),
];

fn graph(switched: &str, before: &str) -> Graph {
    let mut files = sva_ast::Composition::new();
    files
        .insert("tone", "sin(2*pi*220*t) + 0.3*sin(2*pi*661*t)\n")
        .insert("n", switched)
        .insert("b", before)
        .insert("late", "@n(t - 1000sp)\n")
        .insert("late_b", "@b(t - 1000sp)\n");
    sva_ast::load(&files).expect("a composition")
}

fn config() -> RenderConfig {
    RenderConfig::seconds(RATE, SECS)
}

fn whole(g: &Graph, root: &str) -> Vec<f64> {
    let held = render(g, root, config(), None).unwrap_or_else(|e| panic!("{root}: {e}"));
    held.output(held.id(root).expect("a root"))
        .expect("a buffer")
        .plane(0)
        .to_vec()
}

fn streamed(g: &Graph, root: &str, block: usize) -> Vec<f64> {
    let config = StreamConfig {
        block,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some((SECS * f64::from(RATE)) as i64),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let target = sva_ast::parse_expr(&format!("@{root}(t)")).expect("a target");
    let mut stream = Stream::open(g, &target, config, None).unwrap_or_else(|e| panic!("{e}"));
    let mut out = Vec::new();
    while let Some(block) = stream.next_block().unwrap_or_else(|e| panic!("{e}")) {
        out.extend_from_slice(block.plane(0));
    }
    out
}

/// The last switch before which `root`'s prefix identity is `other`'s whole identity.
fn switch(g: &Graph, root: &str, other: &str) -> usize {
    let before = plan(g, other, config()).expect("a plan");
    let want = before
        .prefixes(|walk| walk.prefix_identity(before.id(other).expect("a root"), i64::MAX))
        .expect("an identity");
    let held = plan(g, root, config()).expect("a plan");
    let id = held.id(root).expect("a root");
    held.prefixes(|walk| {
        let points = walk.change_points(id);
        let mut agreeing = points
            .iter()
            .filter(|c| **c > 0)
            .filter(|c| walk.prefix_identity(id, **c).expect("an identity") == want);
        let at = *agreeing
            .next_back()
            .unwrap_or_else(|| panic!("{root} is never {other}: {points:?}"));
        let after = walk.prefix_identity(id, at + 1).expect("an identity");
        assert_ne!(after, want, "{root}: the switch at {at} changes nothing");
        at as usize
    })
}

/// Instants on and off the grid, drawn the same every run.
fn instants() -> Vec<f64> {
    let mut seed: u64 = 0x5eed;
    let mut out = vec![0.25, 0.5, 11_025.5 / f64::from(RATE)];
    for _ in 0..3 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        out.push(0.2 + 0.6 * (seed >> 11) as f64 / (1u64 << 53) as f64);
    }
    out
}

#[test]
fn every_machine_switch_is_its_prefix_form_before_it_whole_and_streamed() {
    for case in MACHINE {
        let (count, blocks) = match case.solver {
            true => (2, &[64, 1_000][..]),
            false => (6, &[1, 64, 1_000, 4_096][..]),
        };
        for r in instants().into_iter().take(count) {
            let g = graph(&(case.switched)(r), case.before);
            for (n, b) in [("n", "b"), ("late", "late_b")] {
                let at = switch(&g, n, b);
                let want = whole(&g, b);
                let label = format!("{} at {r} ({n}, before {at})", case.name);
                assert_eq!(whole(&g, n)[..at], want[..at], "{label}: whole");
                for &block in blocks {
                    let heard = streamed(&g, n, block);
                    assert_eq!(heard[..at], want[..at], "{label}: in blocks of {block}");
                }
            }
        }
    }
}

#[test]
fn a_closed_form_rows_collapse_places_no_switch() {
    for written in ROWS {
        let g = graph(&written(0.5), "0\n");
        let held = plan(&g, "n", config()).expect("a plan");
        let id = held.id("n").expect("a root");
        let points = held.prefixes(|walk| walk.change_points(id));
        assert!(points.is_empty(), "{}: {points:?}", written(0.5));
    }
}
