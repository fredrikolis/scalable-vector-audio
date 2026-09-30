// Concern: proves a term proven under the profile's prune level is zero from there, reported, whole or streamed | Non-concern: deriving a bound | IO: (a composition, a profile) -> samples, the cut

use std::cell::RefCell;

use crate::fixtures::{Now, added, graph_of, next, replaced};
use sva_ast::Graph;
use sva_engine::{
    NoStore, PSYCHOACOUSTIC_V1, Profile, Range, Render, RenderConfig, Stream, StreamConfig,
};

const RATE: u32 = 8_000;
const BLOCK: usize = 256;

fn fades() -> Graph {
    graph_of(
        "faded",
        &[
            ("fade", "sin(2*pi*200*t)*exp(-t/0.15)\n"),
            ("tone", "0.5*sin(2*pi*300*t)\n"),
            ("mix", "@fade(t) + @tone(t)\n"),
            ("echo", "sample(x) + 0.5*self[idx(t - 0.25s)]\n"),
            ("echoed", "@echo(t, x=crop(sin(2*pi*200*t), 0s, 0.1s))\n"),
        ],
    )
}

fn rendered(g: &Graph, target: &str, profile: Profile) -> Render {
    let config = RenderConfig {
        profile,
        ..RenderConfig::seconds(RATE, 4.0)
    };
    sva_engine::render(g, target, config, None).unwrap_or_else(|e| panic!("{target}: {e}"))
}

fn plane(r: &Render) -> Vec<f64> {
    r.output(r.root).expect("the root").plane(0).to_vec()
}

/// Where the render's own label says `node` was cut.
fn cut(r: &Render, node: &str) -> Option<i64> {
    let pruned = r.labels[&r.root]
        .pruned
        .clone()
        .expect("a render states its prune level");
    pruned
        .cuts
        .iter()
        .find(|(n, _)| n == node)
        .map(|(_, at)| *at)
}

fn at(profile_db: f64) -> Profile {
    Profile {
        prune_db: profile_db,
        ..PSYCHOACOUSTIC_V1
    }
}

/// `exp(-t/0.15)` falls under -120 dBFS a little past 2.07 s: the fade is exactly zero from
/// the sample its bound stays under, and a sum reading it is the other addend alone there.
#[test]
fn a_decayed_term_is_zero_from_where_its_bound_stays_under_the_level() {
    let g = fades();
    let fade = rendered(&g, "fade", PSYCHOACOUSTIC_V1);
    let pruned = fade.labels[&fade.root]
        .pruned
        .clone()
        .expect("a stated level");
    assert_eq!(pruned.db, -120.0);
    let from = cut(&fade, "fade").expect("the fade is cut");
    let seconds = from as f64 / f64::from(RATE);
    assert!((2.07..2.2).contains(&seconds), "cut at {seconds} s");
    let samples = plane(&fade);
    let from = usize::try_from(from).expect("a cut inside the render");
    assert!(samples[..from].iter().rev().take(40).any(|v| *v != 0.0));
    assert!(
        samples[from..].iter().all(|v| v.to_bits() == 0),
        "zero past the cut"
    );

    let mix = plane(&rendered(&g, "mix", PSYCHOACOUSTIC_V1));
    let tone = plane(&rendered(&g, "tone", PSYCHOACOUSTIC_V1));
    for n in from..mix.len() {
        assert_eq!(mix[n].to_bits(), tone[n].to_bits(), "sample {n}");
    }
}

/// A sustained tone's bound never falls, and a loop has none: neither is cut.
#[test]
fn a_term_with_no_falling_bound_is_never_pruned() {
    let g = fades();
    let tone = rendered(&g, "tone", PSYCHOACOUSTIC_V1);
    assert_eq!(cut(&tone, "tone"), None);
    assert!(
        plane(&tone)[4 * RATE as usize - 40..]
            .iter()
            .any(|v| *v != 0.0)
    );
    let echoed = rendered(&g, "echoed", PSYCHOACOUSTIC_V1);
    let pruned = echoed.labels[&echoed.root]
        .pruned
        .clone()
        .expect("a stated level");
    assert!(
        pruned
            .cuts
            .iter()
            .all(|(node, _)| !node.starts_with("echo")),
        "{:?}",
        pruned.cuts
    );
}

/// A profile pruning at -60 dBFS cuts the fade at about half the time -120 does.
#[test]
fn the_profile_s_level_moves_the_cut() {
    let g = fades();
    let loud = rendered(&g, "fade", at(-60.0));
    let quiet = rendered(&g, "fade", PSYCHOACOUSTIC_V1);
    assert_eq!(
        loud.labels[&loud.root].pruned.as_ref().map(|p| p.db),
        Some(-60.0)
    );
    let (early, late) = (
        cut(&loud, "fade").expect("cut"),
        cut(&quiet, "fade").expect("cut"),
    );
    let seconds = early as f64 / f64::from(RATE);
    assert!((1.03..1.15).contains(&seconds), "cut at {seconds} s");
    assert!(early < late, "{early} before {late}");
    assert!(plane(&loud)[early as usize..].iter().all(|v| *v == 0.0));
    assert!(
        plane(&quiet)[early as usize..late as usize]
            .iter()
            .any(|v| *v != 0.0)
    );
}

/// A held note let up by a key-up fade leaves `@notes` once its fade stays under -120 dBFS,
/// with no remove, while a note still held sounds on.
#[test]
fn a_faded_key_up_leaves_the_stream_s_sum_with_no_remove() {
    let g = graph_of("faded-stream", &[("pad", "sin(2*pi*f0*t)\n")]);
    let config = StreamConfig {
        block: BLOCK,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(60 * i64::from(RATE)),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let expr = |text: &str| sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("{}", e.message));
    let stream = Stream::open(&g, &expr("@notes"), config, None, &NoStore).now();
    let stream = RefCell::new(stream.expect("opens"));
    let blocks = |count: usize| {
        for _ in 0..count {
            let block = next(&mut stream.borrow_mut()).unwrap_or_else(|e| panic!("{e}"));
            assert!(block.is_some(), "the stream plays on");
        }
    };
    let add = |text: &str| {
        added(&stream, &g, &expr(text), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("{e}"))
    };
    add("@pad(t, f0=300)");
    let note = add("@pad(t, f0=200)");
    blocks(4);
    let up = stream.borrow().position();
    let fade = format!("@pad(t, f0=200)*(1 - step(t - {up}sp)*(1 - exp(-(t - {up}sp)/0.15)))");
    let held = replaced(&stream, &g, (note, &expr(&fade)), &NoStore).now();
    assert_eq!(held.ok(), Some(true), "the note is held");
    let terms = || stream.borrow().counts().terms;
    let per_second = RATE as usize / BLOCK;
    blocks(per_second);
    assert_eq!(terms(), 2, "fading, a second after its key-up");
    blocks(per_second + per_second / 2);
    assert_eq!(terms(), 1, "under the level, 2.5 s after its key-up");
    assert_eq!(stream.borrow().pruned().db, -120.0);
}
