// Concern: proves an edit plays the whole render of the edited expression where nothing before now moved, and carries changed state | Non-concern: the blocks before it | IO: (a stream, exprs) -> blocks

mod fixtures;

use fixtures::graph_of;
use sva_ast::Graph;
use sva_engine::{Cache, Outcome, PayloadKind, Range, RenderConfig, Stream, StreamConfig, render};

const RATE: u32 = 44_100;
const BLOCK: usize = 1_024;

/// piano3 with its damper wired to `release`.
const PIANO: &str = "0.0014822 * chaigne_askenfelt(f0, vel=vel, release=release, \
    b=max(1.4e-4, 4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12, hammer_mass=0.009, \
    hammer_k=2e10*max(1, pow(f0/523, 0.6)), hammer_p=3, damp_dc=1.327*exp(0.394*log(f0/262) \
    - 0.12*log(f0/262)*log(f0/262)), damp_freq=0.00044109413838472024, unison_count=3, \
    detune=1, bridge_coupling=97.2*pow(f0/262, 0.677), bridge_mass=1.04*exp(0.466*log(f0/262) \
    - 0.506*log(f0/262)*log(f0/262)), string1_cents=-0.2, string2_cents=0, string3_cents=0.25, \
    string1_hammer_k_ratio=1, string2_hammer_k_ratio=0.8, string3_hammer_k_ratio=0.6)\n";

const ECHO: &str = "feedback = 0.35\nx + feedback*self(t - 0.25s)\n";

fn composition(released: f64) -> Graph {
    graph_of(
        "edit",
        &[
            ("piano", PIANO),
            ("echo", ECHO),
            ("pluck", "chaigne_askenfelt(f0, release=release)\n"),
            (
                "key",
                "@echo(t, x=@piano(t, f0=261.63, vel=4.5, release=release))\n",
            ),
            ("released", &format!("@key(t, release={released})\n")),
            ("note", "@piano(t, f0=261.63, vel=4.5, release=release)\n"),
            ("played", &format!("@note(t, release={released})\n")),
            (
                "toned",
                "lowpass(highpass(@note(t, release=release), cutoff=180, q=0.7), cutoff=2400, \
                 q=1.3)\n",
            ),
            ("toned_up", &format!("@toned(t, release={released})\n")),
            ("string", "@pluck(t, f0=523.25, release=release)\n"),
            ("struck", &format!("@string(t, release={released})\n")),
            (
                "doubled",
                "@string(t, release=release) + 0.5*@string(t - 0.3s, release=release)\n",
            ),
            ("doubled_up", &format!("@doubled(t, release={released})\n")),
            ("burst", "crop(sin(2*pi*440*t), 0s, 0.05s)\n"),
            (
                "blip",
                "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.4s)\n",
            ),
        ],
    )
}

fn config(end: Option<i64>) -> StreamConfig {
    StreamConfig {
        block: BLOCK,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end,
            },
            ..RenderConfig::at(RATE)
        },
    }
}

fn expr(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("`{text}`: {}", e.message))
}

fn opened(g: &Graph, text: &str, cache: Option<&Cache>) -> Stream {
    let end = Some(4 * i64::from(RATE));
    Stream::open(g, &expr(text), config(end), cache).unwrap_or_else(|e| panic!("{e}"))
}

fn edit(stream: &mut Stream, g: &Graph, text: &str) {
    stream
        .edit(g, &expr(text))
        .unwrap_or_else(|e| panic!("`{text}`: {e}"));
}

fn blocks(stream: &mut Stream, count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count * BLOCK);
    for _ in 0..count {
        let block = stream.next_block().unwrap_or_else(|e| panic!("{e}"));
        out.extend_from_slice(block.expect("a stream with no end").plane(0));
    }
    out
}

fn whole(g: &Graph, target: &str, samples: usize) -> Vec<f64> {
    let secs = samples as f64 / f64::from(RATE);
    let held = render(g, target, RenderConfig::seconds(RATE, secs), None)
        .unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

/// Key-up is an edit binding `release` at sample `k`, on the block grid: the held blocks and
/// the released ones after are one whole render with `release = k/rate`.
fn released_at(target: &str, whole_target: &str, k: usize) {
    let release = k as f64 / f64::from(RATE);
    let g = composition(release);
    let mut held = opened(&g, &format!("@{target}(t)"), None);
    let mut heard = blocks(&mut held, k / BLOCK);
    let mut released = opened(&g, &format!("@{target}(t)"), None);
    blocks(&mut released, k / BLOCK);
    edit(
        &mut released,
        &g,
        &format!("@{target}(t, release={release})"),
    );
    heard.extend(blocks(&mut released, 8));

    let want = whole(&g, whole_target, heard.len());
    assert_ne!(
        want[k + 2_000..],
        blocks(&mut held, 8)[2_000..],
        "{target}: the damper did nothing"
    );
    assert_eq!(heard, want, "{target}");
}

#[test]
fn key_up_over_an_echo_is_the_whole_render_released_there() {
    released_at("key", "released", 12 * BLOCK);
}

#[test]
fn key_up_of_a_unison_under_filters_is_the_whole_render_released_there() {
    released_at("note", "played", 22 * BLOCK);
    released_at("toned", "toned_up", 22 * BLOCK);
}

/// Reading the released node twice at an offset, not the string under it, is what this
/// proves, so the cheap single string carries it.
#[test]
fn key_up_of_a_string_read_again_later_is_the_whole_render_released_there() {
    released_at("string", "struck", 3 * BLOCK);
    released_at("doubled", "doubled_up", 3 * BLOCK);
}

/// A note pressed at `t0` is a term shifted there, and its key-up at `t1` binds that term's
/// `release`: the stream is the whole render of the final expression, echo tail and all. The
/// first note pressed again at `t2` is its own node read a second time, further back than the
/// stream still holds, so it is stepped again from its start.
#[test]
fn a_note_pressed_and_released_mid_stream_is_the_whole_render_of_the_last_edit() {
    let (t0, t1, t2) = (5 * BLOCK, 9 * BLOCK, 12 * BLOCK);
    let release = (t1 - t0) as f64 / f64::from(RATE);
    let first = "@pluck(t, f0=440)";
    let pressed = format!("{first} + @pluck(t - {t0}sp, f0=660)");
    let released = format!("{first} + @pluck(t - {t0}sp, f0=660, release={release})");
    let last = format!("{released} + @pluck(t - {t2}sp, f0=440)");
    let g = composition(1.0);
    let echoed = |x: &str| format!("@echo(t, x={x})");
    let mut stream = opened(&g, &echoed(first), None);
    let mut heard = blocks(&mut stream, t0 / BLOCK);
    edit(&mut stream, &g, &echoed(&pressed));
    heard.extend(blocks(&mut stream, (t1 - t0) / BLOCK));
    edit(&mut stream, &g, &echoed(&released));
    heard.extend(blocks(&mut stream, (t2 - t1) / BLOCK));
    edit(&mut stream, &g, &echoed(&last));
    heard.extend(blocks(&mut stream, 16));

    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&echoed(&last))));
    let want = whole(&whole_g, "final", heard.len());
    assert_ne!(
        want,
        whole(&g, "string", heard.len()),
        "the press was heard"
    );
    assert_eq!(heard, want);
}

/// A term whose node has ended leaves `@notes` with its handle; a player that only adds hears
/// the whole render of every note it gave, echo and all.
#[test]
fn a_note_past_its_end_leaves_the_sum_and_its_echo_rings_on() {
    let g = composition(1.0);
    let mut stream = opened(&g, "@echo(t, x=@notes)", None);
    let (a, b) = ("@blip(t - 1024sp, f0=200)", "@blip(t - 8192sp, f0=300)");
    let added = |stream: &mut Stream, term: &str| {
        stream
            .add(&g, &expr(term))
            .unwrap_or_else(|e| panic!("`{term}`: {e}"))
    };
    let (held_a, held_b) = (added(&mut stream, a), added(&mut stream, b));
    let mut heard = blocks(&mut stream, 20);
    assert_eq!(
        stream.remove(held_a).ok(),
        Some(false),
        "the first note ended"
    );
    heard.extend(blocks(&mut stream, 10));
    let c = "@blip(t - 30720sp, f0=250)";
    added(&mut stream, c);
    assert_eq!(
        stream.remove(held_b).ok(),
        Some(false),
        "the second note ended"
    );
    heard.extend(blocks(&mut stream, 30));

    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&format!("@echo(t, x={a} + {b} + {c})"))));
    let want = whole(&whole_g, "final", heard.len());
    let gap = 8192 + 17_640..30 * BLOCK;
    assert!(
        heard[gap].iter().any(|v| *v != 0.0),
        "the echo rings between notes"
    );
    assert_eq!(heard, want);
}

/// Key-up on a note the store answered finds no state at the edit's instant: an exact stream
/// computes the string again from its start, a live one starts it silent and names it dropped.
#[test]
fn a_live_edit_starts_a_node_with_no_state_silent_and_names_it() {
    let g = composition(1.0);
    let cache = Cache::new();
    render(&g, "string", RenderConfig::seconds(RATE, 0.5), Some(&cache)).expect("a render");
    let held = "@echo(t, x=@pluck(t, f0=523.25))";
    let released = "@echo(t, x=@pluck(t, f0=523.25, release=0.1))";
    let (mut exact, mut live) = (
        opened(&g, held, Some(&cache)),
        opened(&g, held, Some(&cache)),
    );
    live.go_live();
    let before = blocks(&mut live, 8);
    assert_eq!(before, blocks(&mut exact, 8));
    edit(&mut exact, &g, released);
    edit(&mut live, &g, released);
    assert!(exact.dropped().is_empty());
    assert_eq!(live.dropped().len(), 1, "{:?}", live.dropped());
    assert!(
        live.dropped()[0].starts_with("pluck("),
        "{:?}",
        live.dropped()
    );
    assert_ne!(blocks(&mut live, 4), blocks(&mut exact, 4));
}

/// A constant moved inside a loop is a changed node read where its predecessor was: it takes
/// the loop's own past, so the tail rings on under the new constant from the next block.
#[test]
fn a_constant_moved_inside_a_loop_keeps_its_tail_ringing() {
    let g = composition(1.0);
    let mut stream = opened(&g, "@echo(t, x=sample(@burst), feedback=0.35)", None);
    let k = 13 * BLOCK;
    let mut heard = blocks(&mut stream, k / BLOCK);
    edit(&mut stream, &g, "@echo(t, x=sample(@burst), feedback=0.5)");
    heard.extend(blocks(&mut stream, 12));
    let delay = RATE as usize / 4;
    assert!(heard[k..].iter().any(|v| *v != 0.0), "the tail rings on");
    for n in k..heard.len() {
        assert_eq!(heard[n], 0.5 * heard[n - delay], "sample {n}");
    }
}

/// An hour's playing in miniature: a note every eight blocks, each gone from the expression
/// two seconds after it starts. Past its extent a note holds nothing, so what the stream holds
/// levels off within the first seconds and never climbs past that.
#[test]
fn a_long_session_holds_no_more_than_its_first_seconds() {
    let rate = 8_000;
    let g = composition(1.0);
    let config = StreamConfig {
        block: 256,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(3_600 * i64::from(rate)),
            },
            ..RenderConfig::at(rate)
        },
    };
    let mut stream = Stream::open(&g, &expr("@echo(t, x=0)"), config, None).expect("opens");
    let (every, kept) = (8 * 256, 2 * i64::from(rate));
    let mut onsets: Vec<i64> = Vec::new();
    let mut held = Vec::new();
    while stream.position() < 60 * i64::from(rate) {
        let now = stream.position();
        if now % every == 0 {
            onsets.retain(|onset| now - onset < kept);
            onsets.push(now);
            let notes: Vec<String> = onsets
                .iter()
                .map(|at| format!("@blip(t - {at}sp, f0={})", 200 + at % 7 * 50))
                .collect();
            edit(
                &mut stream,
                &g,
                &format!("@echo(t, x={})", notes.join(" + ")),
            );
        }
        stream.next_block().expect("a block").expect("no end");
        held.push(stream.held_bytes());
    }
    let seconds = |s: usize| s * rate as usize / 256;
    let first = held[..seconds(10)].iter().max().expect("ten seconds");
    let last = held[seconds(50)..].iter().max().expect("ten more");
    assert!(last <= first, "{last} bytes held late, {first} early");
}

/// A shifted term reads the node the store holds by identity, wherever it is shifted to: a
/// shorter run is carried on from its end and stored again, and a run long enough answers
/// both of two shifted reads of the one node. Either way, the samples are the cold stream's.
#[test]
fn a_shifted_term_reads_and_extends_the_run_its_node_holds_in_the_store() {
    let g = composition(1.0);
    let cache = Cache::new();
    render(&g, "string", RenderConfig::seconds(RATE, 0.2), Some(&cache)).expect("a render");
    let runs = |stream: &Stream, outcome: Outcome| {
        let stats = stream.stats();
        let lookups = stats.lookups.iter();
        lookups
            .filter(|l| l.kind == PayloadKind::Run && l.outcome == outcome)
            .count()
    };
    let later = "@echo(t, x=@pluck(t - 2000sp, f0=523.25))";
    let (mut cold, mut warm) = (opened(&g, later, None), opened(&g, later, Some(&cache)));
    assert_eq!(blocks(&mut warm, 20), blocks(&mut cold, 20));
    edit(&mut warm, &g, "@echo(t, x=0)");
    assert_eq!(runs(&warm, Outcome::Extended), 1, "{:?}", warm.stats());

    let twice = "@pluck(t - 1000sp, f0=523.25) + @pluck(t - 3000sp, f0=523.25)";
    let (mut cold, mut warm) = (opened(&g, twice, None), opened(&g, twice, Some(&cache)));
    assert_eq!(blocks(&mut warm, 16), blocks(&mut cold, 16));
    assert_eq!(runs(&warm, Outcome::Hit), 1, "{:?}", warm.stats());
}
