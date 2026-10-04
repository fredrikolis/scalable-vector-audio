// Concern: proves an open render, or a stream's term, ends where its bound at the root is under the prune level | Non-concern: deriving a bound | IO: (a composition, a profile) -> samples, the cut

use std::cell::RefCell;

use crate::fixtures::{Now, added, graph_of, next, replaced};
use sva_ast::Graph;
use sva_engine::{
    PSYCHOACOUSTIC_V1, Profile, Range, Render, RenderConfig, Stream, StreamConfig, Tier,
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
        ..RenderConfig::at(RATE)
    };
    sva_engine::render(g, target, config, &Tier::default())
        .unwrap_or_else(|e| panic!("{target}: {e}"))
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

/// `exp(-t/0.15)` falls under -120 dBFS a little past 2.07 s: an open render of the fade ends
/// at the sample its bound stays under, and says so.
#[test]
fn a_decayed_sound_ends_where_its_bound_stays_under_the_level() {
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
    assert_eq!(samples.len() as i64, from, "it ends at its cut");
    assert!(samples.iter().rev().take(40).any(|v| *v != 0.0));
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
    assert_eq!(plane(&loud).len() as i64, early);
    assert!(
        plane(&quiet)[early as usize..late as usize]
            .iter()
            .any(|v| *v != 0.0)
    );
}

/// A closed interval renders its whole length: no node is cut under it, however quiet.
#[test]
fn a_closed_interval_cuts_nothing() {
    let g = fades();
    let closed = RenderConfig {
        profile: PSYCHOACOUSTIC_V1,
        ..RenderConfig::seconds(RATE, 4.0)
    };
    let mix = sva_engine::render(&g, "mix", closed, &Tier::default()).expect("a render");
    let pruned = mix.labels[&mix.root]
        .pruned
        .clone()
        .expect("a stated level");
    assert!(pruned.cuts.is_empty(), "{:?}", pruned.cuts);
    let exact = RenderConfig {
        profile: at(f64::NEG_INFINITY),
        ..RenderConfig::seconds(RATE, 4.0)
    };
    let unpruned = sva_engine::render(&g, "mix", exact, &Tier::default()).expect("a render");
    let (a, b) = (plane(&mix), plane(&unpruned));
    assert!(a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits()));
}

/// A held note let up by a key-up fade leaves `@notes` once its fade, heard at the root, stays
/// under -120 dBFS, with no remove, while a note still held sounds on: read bare, or through a
/// filter whose gain the bound carries.
#[test]
fn a_faded_key_up_leaves_the_stream_s_sum_with_no_remove() {
    for target in ["@notes", "2*lowpass(sample(@notes), cutoff=3000)"] {
        let g = graph_of("faded-stream", &[("pad", "sin(2*pi*f0*t)\n")]);
        let config = StreamConfig {
            block: BLOCK,
            channels: None,
            render: RenderConfig {
                range: Range {
                    start: Some(0),
                    end: Some(60 * i64::from(RATE)),
                },
                ..RenderConfig::at(RATE)
            },
        };
        let expr =
            |text: &str| sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("{}", e.message));
        let stream = Stream::open(&g, &expr(target), config, &Tier::default()).now();
        let stream = RefCell::new(stream.expect("opens"));
        let blocks = |count: usize| {
            for _ in 0..count {
                let block = next(&mut stream.borrow_mut()).unwrap_or_else(|e| panic!("{e}"));
                assert!(block.is_some(), "the stream plays on");
            }
        };
        let add = |text: &str| {
            added(&stream, &g, &expr(text), &Tier::default())
                .now()
                .unwrap_or_else(|e| panic!("{e}"))
        };
        add("@pad(t, f0=300)");
        let note = add("@pad(t, f0=200)");
        blocks(4);
        let up = stream.borrow().position();
        let fade = format!("@pad(t, f0=200)*(1 - step(t - {up}sp)*(1 - exp(-(t - {up}sp)/0.15)))");
        let held = replaced(&stream, &g, (note, &expr(&fade)), &Tier::default()).now();
        assert_eq!(held.ok(), Some(true), "the note is held");
        let terms = || stream.borrow().counts().terms;
        let per_second = RATE as usize / BLOCK;
        blocks(per_second);
        assert_eq!(terms(), 2, "{target}: fading, a second after its key-up");
        blocks(per_second + per_second / 2);
        assert_eq!(
            terms(),
            1,
            "{target}: under the level, 2.5 s after its key-up"
        );
        assert_eq!(stream.borrow().pruned().db, -120.0);
    }
}

/// Notes each falling under the level leave the sum early only while all that left early,
/// summed at the root, stay under it: decaying notes sounding as one, every 32nd of a second,
/// differ from the stream with cuts off by less than the level, and few sound at once.
#[test]
fn notes_let_go_early_stay_under_the_level_together() {
    let g = graph_of("faded-together", &[("pad", "sin(2*pi*f0*t)\n")]);
    let played = |prune_db: f64| {
        let config = StreamConfig {
            block: 50,
            channels: None,
            render: RenderConfig {
                profile: at(prune_db),
                ..RenderConfig::at(RATE)
            },
        };
        let expr =
            |text: &str| sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("{}", e.message));
        let stream = Stream::open(&g, &expr("@notes"), config, &Tier::default()).now();
        let stream = RefCell::new(stream.expect("opens"));
        let (mut samples, mut most) = (Vec::new(), 0);
        for _ in 0..160 {
            let at = stream.borrow().position();
            let note = expr(&format!("0.2*exp(-(t - {at}sp)/0.05)"));
            added(&stream, &g, &note, &Tier::default())
                .now()
                .unwrap_or_else(|e| panic!("{e}"));
            for _ in 0..5 {
                let block = next(&mut stream.borrow_mut()).unwrap_or_else(|e| panic!("{e}"));
                samples.extend_from_slice(block.expect("plays on").plane(0));
            }
            most = most.max(stream.borrow().counts().terms);
        }
        (samples, most, stream.borrow().counts().terms)
    };
    let (pruned, most, _) = played(-120.0);
    let (exact, _, kept) = played(f64::NEG_INFINITY);
    assert_eq!(kept, 160, "with cuts off every note stays");
    assert!(most < 40, "{most} sound at once");
    let moved = pruned.iter().zip(&exact).map(|(a, b)| (a - b).abs());
    let moved = moved.fold(0.0f64, f64::max);
    assert!(moved > 0.0 && moved < 1e-6, "the root moved {moved}");
}
