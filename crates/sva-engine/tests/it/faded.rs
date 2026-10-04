// Concern: proves an open render ends where its root is proven under the profile's prune level, and reports it | Non-concern: deriving a bound | IO: (a composition, a profile) -> samples, the cut

use crate::fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{PSYCHOACOUSTIC_V1, Profile, Render, RenderConfig, Tier};

const RATE: u32 = 8_000;

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
