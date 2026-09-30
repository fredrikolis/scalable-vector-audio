// Concern: proves an edit plays the whole render of the edited expression where nothing before now moved, and carries changed state | Non-concern: the blocks before it | IO: (a stream, exprs) -> blocks

use crate::fixtures::{Now, graph_of};
use sva_ast::Graph;
use sva_engine::{
    Cache, NoStore, Outcome, PayloadKind, Range, RenderConfig, Stream, StreamConfig, render,
};

const RATE: u32 = 8_000;
const BLOCK: usize = 256;

/// piano3 with its damper wired to `release`: lifted until then, ramped in over 0.03 s.
const PIANO: &str = "release = inf\n0.0014822 * chaigne_askenfelt(f0, vel=vel, \
    damper_r=0.1*pow(262/f0, 2)*crop(min(1, (t - release)/0.03), release, inf), \
    b=max(1.4e-4, 4.1e-4*pow(f0/262, 1.9)), strike_pos=0.12, hammer_mass=0.009, \
    hammer_k=2e10*max(1, pow(f0/523, 0.6)), hammer_p=3, damp_dc=1.327*exp(0.394*log(f0/262) \
    - 0.12*log(f0/262)*log(f0/262)), damp_freq=0.00044109413838472024, unison_count=3, \
    detune=1, bridge_coupling=97.2*pow(f0/262, 0.677), bridge_mass=1.04*exp(0.466*log(f0/262) \
    - 0.506*log(f0/262)*log(f0/262)), string1_cents=-0.2, string2_cents=0, string3_cents=0.25, \
    string1_hammer_k_ratio=1, string2_hammer_k_ratio=0.8, string3_hammer_k_ratio=0.6)\n";

const PLUCK: &str = "release = inf\nchaigne_askenfelt(f0, damper_r=0.1*pow(262/f0, 2)\
    *crop(min(1, (t - release)/0.03), release, inf))\n";

const ECHO: &str = "feedback = 0.35\nsample(x) + feedback*self[idx(t - 0.25s)]\n";

fn composition(released: f64) -> Graph {
    graph_of(
        "edit",
        &[
            ("piano", PIANO),
            ("echo", ECHO),
            ("pluck", PLUCK),
            (
                "key",
                "release = inf\n@echo(t, x=@piano(t, f0=261.63, vel=4.5, release=release))\n",
            ),
            ("released", &format!("@key(t, release={released})\n")),
            (
                "note",
                "release = inf\n@piano(t, f0=261.63, vel=4.5, release=release)\n",
            ),
            ("played", &format!("@note(t, release={released})\n")),
            (
                "toned",
                "release = inf\nlowpass(highpass(@note(t, release=release), cutoff=180, q=0.7), cutoff=2400, \
                 q=1.3)\n",
            ),
            ("toned_up", &format!("@toned(t, release={released})\n")),
            (
                "string",
                "release = inf\n@pluck(t, f0=523.25, release=release)\n",
            ),
            ("struck", &format!("@string(t, release={released})\n")),
            (
                "doubled",
                "release = inf\n@string(t, release=release) + 0.5*@string(t - 0.3s, release=release)\n",
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

fn config_at(rate: u32, end: Option<i64>) -> StreamConfig {
    StreamConfig {
        block: BLOCK,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end,
            },
            ..RenderConfig::at(rate)
        },
    }
}

fn expr(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).unwrap_or_else(|e| panic!("`{text}`: {}", e.message))
}

fn opened(g: &Graph, text: &str, cache: Option<&Cache>) -> Stream {
    opened_at(RATE, g, text, cache)
}

fn opened_at(rate: u32, g: &Graph, text: &str, cache: Option<&Cache>) -> Stream {
    let end = Some(4 * i64::from(rate));
    Stream::open(g, &expr(text), config_at(rate, end), cache, &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"))
}

fn edit(stream: &mut Stream, g: &Graph, text: &str) {
    stream
        .edit(g, &expr(text), &NoStore)
        .now()
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
    whole_at(RATE, g, target, samples)
}

fn whole_at(rate: u32, g: &Graph, target: &str, samples: usize) -> Vec<f64> {
    let secs = samples as f64 / f64::from(rate);
    let held = render(g, target, RenderConfig::seconds(rate, secs), None)
        .unwrap_or_else(|e| panic!("{target}: {e}"));
    let id = held.id(target).expect("the root");
    held.output(id).expect("a buffer").plane(0).to_vec()
}

/// Key-up is an edit binding `release` at sample `k`, on the block grid: the held blocks and
/// the released ones after are one whole render with `release = k/rate`.
fn released_at(target: &str, whole_target: &str, k: usize) {
    released_at_rate(RATE, target, whole_target, k);
}

fn released_at_rate(rate: u32, target: &str, whole_target: &str, k: usize) {
    let release = k as f64 / f64::from(rate);
    let g = composition(release);
    let mut held = opened_at(rate, &g, &format!("@{target}(t)"), None);
    let mut heard = blocks(&mut held, k / BLOCK);
    let mut released = opened_at(rate, &g, &format!("@{target}(t)"), None);
    blocks(&mut released, k / BLOCK);
    edit(
        &mut released,
        &g,
        &format!("@{target}(t, release={release})"),
    );
    heard.extend(blocks(&mut released, 8));

    let want = whole_at(rate, &g, whole_target, heard.len());
    assert_ne!(
        want[k + 2 * BLOCK..],
        blocks(&mut held, 8)[2 * BLOCK..],
        "{target}: the damper did nothing"
    );
    assert_eq!(heard, want, "{target} at {rate}");
}

#[test]
fn key_up_at_another_rate_is_the_whole_render_released_there() {
    released_at_rate(11_025, "string", "struck", 3 * BLOCK);
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
    let a = format!("@blip(t - {BLOCK}sp, f0=200)");
    let b = format!("@blip(t - {}sp, f0=300)", 8 * BLOCK);
    let added = |stream: &mut Stream, term: &str| {
        stream
            .add(&g, &expr(term), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("`{term}`: {e}"))
    };
    let (held_a, held_b) = (added(&mut stream, &a), added(&mut stream, &b));
    let mut heard = blocks(&mut stream, 20);
    assert_eq!(
        stream.remove(held_a, &NoStore).now().ok(),
        Some(false),
        "the first note ended"
    );
    heard.extend(blocks(&mut stream, 10));
    let c = format!("@blip(t - {}sp, f0=250)", 30 * BLOCK);
    added(&mut stream, &c);
    assert_eq!(
        stream.remove(held_b, &NoStore).now().ok(),
        Some(false),
        "the second note ended"
    );
    heard.extend(blocks(&mut stream, 30));

    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&format!("@echo(t, x={a} + {b} + {c})"))));
    let want = whole(&whole_g, "final", heard.len());
    let gap = 8 * BLOCK + (0.4 * f64::from(RATE)) as usize..30 * BLOCK;
    assert!(
        heard[gap].iter().any(|v| *v != 0.0),
        "the echo rings between notes"
    );
    assert_eq!(heard, want);
}

/// A term removed while it sounds is cut where the stream stands: what it played stays, and
/// the stream is the whole render of the terms it holds, the cut one among them.
#[test]
fn a_term_removed_while_it_sounds_is_cut_where_the_stream_stands() {
    let g = composition(1.0);
    let mut stream = opened(&g, "@echo(t, x=@notes)", None);
    let mut added = |term: &str| {
        stream
            .add(&g, &expr(term), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("`{term}`: {e}"))
    };
    let first = added(&format!("@blip(t - {BLOCK}sp, f0=200)"));
    added(&format!("@blip(t - {}sp, f0=300)", 4 * BLOCK));
    let mut heard = blocks(&mut stream, 6);
    assert_eq!(stream.remove(first, &NoStore).now().ok(), Some(true));
    assert_eq!(
        stream.remove(first, &NoStore).now().ok(),
        Some(false),
        "a removed handle"
    );
    let held: Vec<String> = stream.exprs().skip(1).map(sva_ast::render_expr).collect();
    heard.extend(blocks(&mut stream, 30));

    let mut whole_g = g.clone();
    let last = format!("@echo(t, x={})", held.join(" + "));
    assert!(whole_g.define("final", expr(&last)));
    assert_eq!(heard, whole(&whole_g, "final", heard.len()), "{last}");
}

/// Key-up on a note the store answered finds no state at the edit's instant, only one marked
/// before it: an exact stream computes the string on from there, a live one computes none of
/// its past and starts it silent, named dropped.
#[test]
fn a_live_edit_starts_a_node_with_no_state_silent_and_names_it() {
    let g = composition(1.0);
    let cache = Cache::new();
    cache.set_mark_every(BLOCK);
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
    let rate = RATE;
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
    let mut stream = Stream::open(&g, &expr("@echo(t, x=0)"), config, None, &NoStore)
        .now()
        .expect("opens");
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
    assert_eq!(
        runs(&warm, Outcome::Hit),
        2,
        "the store answers the first read and the second reuses it: {:?}",
        warm.stats()
    );
}

const COMB: &str = "delay = 0.0297\ng = 0.8\nx + g*self[idx(t - delay)]\n";
const ALLPASS: &str = "delay = 1/300\ng = 0.7\n@comb(t - delay, x=x, delay=delay, g=g) - g*@comb(t, x=x, delay=delay, g=g)\n";
const HALL: &str = "size = 1\ndecay = 0.8\n@allpass(t, x=@allpass(t, x=0.25*(@comb(t, x=x, \
    delay=0.0297*size, g=decay) + @comb(t, x=x, delay=0.0371*size, g=decay)), delay=2/300, g=0.7), \
    delay=1/300, g=0.7)\n";
const SPACE: &str = "mix = 0.3\ndry = x\n(1 - mix)*x + mix*@hall(t, x=dry, size=1, decay=0.8)\n";

fn reverb() -> Graph {
    graph_of(
        "reverb",
        &[
            ("comb", COMB),
            ("allpass", ALLPASS),
            ("hall", HALL),
            ("space", SPACE),
            (
                "blip",
                "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.4s)\n",
            ),
        ],
    )
}

/// Two notes under a master, the second added live: the whole render of both, nothing dropped.
fn added_under(master: &dyn Fn(&str) -> String) {
    let g = reverb();
    let mut stream = opened(&g, &master("@notes"), None);
    stream.go_live();
    let a = format!("@blip(t - {BLOCK}sp, f0=200)");
    let b = format!("@blip(t - {}sp, f0=300)", 8 * BLOCK);
    stream
        .add(&g, &expr(&a), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut heard = blocks(&mut stream, 8);
    stream
        .add(&g, &expr(&b), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    heard.extend(blocks(&mut stream, 12));
    assert!(stream.dropped().is_empty(), "{:?}", stream.dropped());

    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&master(&format!("{a} + {b}")))));
    assert_eq!(heard, whole(&whole_g, "final", heard.len()));
}

#[test]
fn a_note_added_under_a_reverb_keeps_its_tail_ringing() {
    added_under(&|x| format!("@space(t, x=sample({x}), dry=sample({x}), mix=0.3)"));
}

#[test]
fn a_note_added_under_parallel_combs_reads_each_comb_s_own_past() {
    added_under(&|x| format!("@hall(t, x=sample({x}))"));
}

/// A loop's delay lengthened mid-stream reads further back than its predecessor kept: the
/// predecessor is not carried, so an exact stream steps the loop again from its start and
/// plays the whole render of the edit, and a live one starts it silent, named dropped.
#[test]
fn a_loop_edited_to_read_further_back_than_it_kept_steps_again_or_starts_silent() {
    let g = reverb();
    let comb = |delay: f64| format!("@comb(t, x=sample(@blip(t, f0=200)), delay={delay})");
    let k = 4 * BLOCK;
    let mut exact = opened(&g, &comb(0.0297), None);
    let mut heard = blocks(&mut exact, k / BLOCK);
    edit(&mut exact, &g, &comb(0.0371));
    heard.extend(blocks(&mut exact, 8));
    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&comb(0.0371))));
    let want = whole(&whole_g, "final", heard.len());
    assert_eq!(heard[k..], want[k..]);

    let mut live = opened(&g, &comb(0.0297), None);
    live.go_live();
    blocks(&mut live, k / BLOCK);
    edit(&mut live, &g, &comb(0.0371));
    blocks(&mut live, 8);
    assert_eq!(live.dropped().len(), 1, "{:?}", live.dropped());
}

/// With no note playing, `@notes` is the number zero, and the loops over it type.
#[test]
fn a_reverb_over_the_bare_note_sum_opens_with_no_notes_and_plays_them() {
    added_under(&|x| format!("@space(t, x={x}, dry={x}, mix=0.3)"));
}

/// piano4's damper: the fitted value times a ramp clamped to [0, 1] that opens at `release`.
const PIANO4: &str = "release = inf\nvel = 0.5\n0.0014822 * chaigne_askenfelt(f0, vel=1 + 5*vel, \
    damper_r=0.1*pow(262/f0, 2)*min(1, max(0, 1 + 17.312340490667562*log(1318.5102276514797/f0)))\
    *crop(max(0, min(1, (t - release)/0.03)), release, inf))\n";

/// Key-up at a whole `sp` sample is the instant the ramp opens: the clamp holds it at zero
/// there, so the solver takes it, whole, streamed, and replaced live on a held term alike.
#[test]
fn key_up_at_a_whole_sample_opens_a_clamped_damper_ramp_at_zero() {
    let rate = 48_000;
    let (k, after) = (12 * BLOCK, 4 * BLOCK);
    let g = graph_of("key-up-sp", &[("piano4", PIANO4)]);
    let held = "@piano4(t, f0=293.6648, vel=0.7087)";
    let released = format!("@piano4(t, f0=293.6648, vel=0.7087, release={k}sp)");
    let mut whole_g = g.clone();
    assert!(whole_g.define("released", expr(&released)));
    let want = whole_at(rate, &whole_g, "released", k + after);

    let mut stream = opened_at(rate, &g, &released, None);
    assert_eq!(blocks(&mut stream, (k + after) / BLOCK), want, "streamed");

    let mut live = opened_at(rate, &g, "@notes", None);
    let note = live
        .add(&g, &expr(held), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut heard = blocks(&mut live, k / BLOCK);
    let replaced = live
        .replace(&g, (note, &expr(&released)), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(replaced, "the held note is still sounding");
    heard.extend(blocks(&mut live, after / BLOCK));
    assert_eq!(heard, want, "replaced live");
}
