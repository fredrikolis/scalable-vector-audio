// Concern: proves an edit plays the whole render of the edited expression where nothing before now moved, and carries changed state | Non-concern: the blocks before it | IO: (a stream, exprs) -> blocks

use std::cell::RefCell;

use crate::fixtures::{Now, added, edited, graph_of, next, removed, replaced};
use sva_ast::Graph;
use sva_engine::{
    Cache, NoStore, Outcome, PayloadKind, Range, RenderConfig, Stream, StreamConfig, render,
};
use sva_engine::{Change, Changed, Placed, change};

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

fn opened(g: &Graph, text: &str, cache: Option<&Cache>) -> RefCell<Stream> {
    opened_at(RATE, g, text, cache)
}

fn opened_at(rate: u32, g: &Graph, text: &str, cache: Option<&Cache>) -> RefCell<Stream> {
    let end = Some(4 * i64::from(rate));
    let opened = Stream::open(g, &expr(text), config_at(rate, end), cache, &NoStore).now();
    RefCell::new(opened.unwrap_or_else(|e| panic!("{e}")))
}

fn edit(stream: &RefCell<Stream>, g: &Graph, text: &str) {
    edited(stream, g, &expr(text), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("`{text}`: {e}"));
}

fn blocks(stream: &RefCell<Stream>, count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count * BLOCK);
    for _ in 0..count {
        let block = next(&mut stream.borrow_mut()).unwrap_or_else(|e| panic!("{e}"));
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
    let held = opened_at(rate, &g, &format!("@{target}(t)"), None);
    let mut heard = blocks(&held, k / BLOCK);
    let released = opened_at(rate, &g, &format!("@{target}(t)"), None);
    blocks(&released, k / BLOCK);
    edit(&released, &g, &format!("@{target}(t, release={release})"));
    heard.extend(blocks(&released, 8));

    let want = whole_at(rate, &g, whole_target, heard.len());
    assert_ne!(
        want[k + 2 * BLOCK..],
        blocks(&held, 8)[2 * BLOCK..],
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
    let stream = opened(&g, &echoed(first), None);
    let mut heard = blocks(&stream, t0 / BLOCK);
    edit(&stream, &g, &echoed(&pressed));
    heard.extend(blocks(&stream, (t1 - t0) / BLOCK));
    edit(&stream, &g, &echoed(&released));
    heard.extend(blocks(&stream, (t2 - t1) / BLOCK));
    edit(&stream, &g, &echoed(&last));
    heard.extend(blocks(&stream, 16));

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
    let stream = opened(&g, "@echo(t, x=@notes)", None);
    let a = format!("@blip(t - {BLOCK}sp, f0=200)");
    let b = format!("@blip(t - {}sp, f0=300)", 8 * BLOCK);
    let add = |term: &str| {
        added(&stream, &g, &expr(term), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("`{term}`: {e}"))
    };
    let (held_a, held_b) = (add(&a), add(&b));
    let mut heard = blocks(&stream, 20);
    assert_eq!(
        removed(&stream, held_a, &NoStore).now().ok(),
        Some(false),
        "the first note ended"
    );
    heard.extend(blocks(&stream, 10));
    let c = format!("@blip(t - {}sp, f0=250)", 30 * BLOCK);
    add(&c);
    assert_eq!(
        removed(&stream, held_b, &NoStore).now().ok(),
        Some(false),
        "the second note ended"
    );
    heard.extend(blocks(&stream, 30));

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

/// A term removed while it sounds is cut where the stream stands and leaves the sum there:
/// what it played stays, echo and all, and the stream is the whole render of it cut there.
#[test]
fn a_term_removed_while_it_sounds_is_cut_where_the_stream_stands() {
    let g = composition(1.0);
    let stream = opened(&g, "@echo(t, x=@notes)", None);
    let add = |term: &str| {
        added(&stream, &g, &expr(term), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("`{term}`: {e}"))
    };
    let (a, b) = (
        format!("@blip(t - {BLOCK}sp, f0=200)"),
        format!("@blip(t - {}sp, f0=300)", 4 * BLOCK),
    );
    let first = add(&a);
    add(&b);
    let mut heard = blocks(&stream, 6);
    assert_eq!(removed(&stream, first, &NoStore).now().ok(), Some(true));
    assert_eq!(
        removed(&stream, first, &NoStore).now().ok(),
        Some(false),
        "a removed handle"
    );
    assert_eq!(
        stream.borrow().counts().terms,
        1,
        "the cut term left the sum"
    );
    heard.extend(blocks(&stream, 30));

    let cut = (6 * BLOCK) as f64 / f64::from(RATE);
    let mut whole_g = g.clone();
    let last = format!("@echo(t, x=crop({a}, -inf, {cut}s) + {b})");
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
    let (exact, live) = (
        opened(&g, held, Some(&cache)),
        opened(&g, held, Some(&cache)),
    );
    live.borrow_mut().go_live();
    let before = blocks(&live, 8);
    assert_eq!(before, blocks(&exact, 8));
    edit(&exact, &g, released);
    edit(&live, &g, released);
    assert!(exact.borrow().dropped().is_empty());
    assert_eq!(
        live.borrow().dropped().len(),
        1,
        "{:?}",
        live.borrow().dropped()
    );
    assert!(
        live.borrow().dropped()[0].starts_with("pluck("),
        "{:?}",
        live.borrow().dropped()
    );
    assert_ne!(blocks(&live, 4), blocks(&exact, 4));
}

/// A constant moved inside a loop is a changed node read where its predecessor was: it takes
/// the loop's own past, so the tail rings on under the new constant from the next block.
#[test]
fn a_constant_moved_inside_a_loop_keeps_its_tail_ringing() {
    let g = composition(1.0);
    let stream = opened(&g, "@echo(t, x=sample(@burst), feedback=0.35)", None);
    let k = 13 * BLOCK;
    let mut heard = blocks(&stream, k / BLOCK);
    edit(&stream, &g, "@echo(t, x=sample(@burst), feedback=0.5)");
    heard.extend(blocks(&stream, 12));
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
    let stream = Stream::open(&g, &expr("@echo(t, x=0)"), config, None, &NoStore).now();
    let stream = RefCell::new(stream.expect("opens"));
    let (every, kept) = (8 * 256, 2 * i64::from(rate));
    let mut onsets: Vec<i64> = Vec::new();
    let mut held = Vec::new();
    while stream.borrow().position() < 60 * i64::from(rate) {
        let now = stream.borrow().position();
        if now % every == 0 {
            onsets.retain(|onset| now - onset < kept);
            onsets.push(now);
            let notes: Vec<String> = onsets
                .iter()
                .map(|at| format!("@blip(t - {at}sp, f0={})", 200 + at % 7 * 50))
                .collect();
            edit(&stream, &g, &format!("@echo(t, x={})", notes.join(" + ")));
        }
        let block = next(&mut stream.borrow_mut()).expect("a block");
        block.expect("no end");
        held.push(stream.borrow().held_bytes());
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
    let runs = |stream: &RefCell<Stream>, outcome: Outcome| {
        let stats = stream.borrow().stats();
        let lookups = stats.lookups.iter();
        lookups
            .filter(|l| l.kind == PayloadKind::Run && l.outcome == outcome)
            .count()
    };
    let later = "@echo(t, x=@pluck(t - 2000sp, f0=523.25))";
    let (cold, warm) = (opened(&g, later, None), opened(&g, later, Some(&cache)));
    assert_eq!(blocks(&warm, 20), blocks(&cold, 20));
    edit(&warm, &g, "@echo(t, x=0)");
    assert_eq!(
        runs(&warm, Outcome::Extended),
        1,
        "{:?}",
        warm.borrow().stats()
    );

    let twice = "@pluck(t - 1000sp, f0=523.25) + @pluck(t - 3000sp, f0=523.25)";
    let (cold, warm) = (opened(&g, twice, None), opened(&g, twice, Some(&cache)));
    assert_eq!(blocks(&warm, 16), blocks(&cold, 16));
    assert_eq!(
        runs(&warm, Outcome::Hit),
        2,
        "the store answers the first read and the second reuses it: {:?}",
        warm.borrow().stats()
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
    let stream = opened(&g, &master("@notes"), None);
    stream.borrow_mut().go_live();
    let a = format!("@blip(t - {BLOCK}sp, f0=200)");
    let b = format!("@blip(t - {}sp, f0=300)", 8 * BLOCK);
    added(&stream, &g, &expr(&a), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut heard = blocks(&stream, 8);
    added(&stream, &g, &expr(&b), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    heard.extend(blocks(&stream, 12));
    assert!(
        stream.borrow().dropped().is_empty(),
        "{:?}",
        stream.borrow().dropped()
    );

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
    let exact = opened(&g, &comb(0.0297), None);
    let mut heard = blocks(&exact, k / BLOCK);
    edit(&exact, &g, &comb(0.0371));
    heard.extend(blocks(&exact, 8));
    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&comb(0.0371))));
    let want = whole(&whole_g, "final", heard.len());
    assert_eq!(heard[k..], want[k..]);

    let live = opened(&g, &comb(0.0297), None);
    live.borrow_mut().go_live();
    blocks(&live, k / BLOCK);
    edit(&live, &g, &comb(0.0371));
    blocks(&live, 8);
    assert_eq!(
        live.borrow().dropped().len(),
        1,
        "{:?}",
        live.borrow().dropped()
    );
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

    let stream = opened_at(rate, &g, &released, None);
    assert_eq!(blocks(&stream, (k + after) / BLOCK), want, "streamed");

    let live = opened_at(rate, &g, "@notes", None);
    let note = added(&live, &g, &expr(held), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut heard = blocks(&live, k / BLOCK);
    let replaced = replaced(&live, &g, (note, &expr(&released)), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(replaced, "the held note is still sounding");
    heard.extend(blocks(&live, after / BLOCK));
    assert_eq!(heard, want, "replaced live");
}

/// A pad held until `release`, then fading, its support running on to the hour.
const PAD: &str = "release = inf\ncrop(sin(2*pi*f0*t)*exp(-t/0.25), 0s, release) + \
    crop(exp(-release/0.25)*sin(2*pi*f0*t)*exp(-(t - release)/0.05), release, 3600s)\n";

/// A player that strikes, lets up and removes each note once faded: `@notes` sums only the
/// notes sounding and fading, however many notes went before.
#[test]
fn a_removed_term_leaves_the_sum_once_the_stream_passes_its_cut() {
    let g = graph_of("pads", &[("pad", PAD)]);
    let config = StreamConfig {
        block: BLOCK,
        render: RenderConfig {
            range: Range {
                start: Some(0),
                end: Some(3_600 * i64::from(RATE)),
            },
            ..RenderConfig::at(RATE)
        },
    };
    let stream = Stream::open(&g, &expr("@notes"), config, None, &NoStore).now();
    let stream = RefCell::new(stream.expect("opens"));
    let fade = (0.2 * f64::from(RATE)) as i64;
    let mut sounding: Vec<(sva_engine::Handle, i64)> = Vec::new();
    for i in 0..300 {
        let onset = stream.borrow().position();
        let pad = format!("@pad(t - {onset}sp, f0={})", 200 + i % 4 * 50);
        let note = added(&stream, &g, &expr(&pad), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("{e}"));
        blocks(&stream, 2);
        let up = stream.borrow().position();
        let released = format!(
            "@pad(t - {onset}sp, f0={}, release={}sp)",
            200 + i % 4 * 50,
            up - onset
        );
        let held = replaced(&stream, &g, (note, &expr(&released)), &NoStore).now();
        assert_eq!(held.ok(), Some(true), "note {i} is held");
        sounding.push((note, up + fade));
        blocks(&stream, 2);
        let now = stream.borrow().position();
        while sounding.first().is_some_and(|(_, ends)| *ends <= now) {
            let (gone, _) = sounding.remove(0);
            let removed = removed(&stream, gone, &NoStore).now().ok();
            assert_eq!(removed, Some(true), "note {i}'s predecessor");
        }
        let terms = stream.borrow().counts().terms;
        assert!(
            terms <= sounding.len(),
            "{terms} terms after note {i}, {} sounding",
            sounding.len()
        );
    }
}

/// A note whose own crop ends leaves the sum once the stream passes that end, removed or
/// not, closed form or sampled, and one cropped wholly into the past leaves as it lands.
#[test]
fn a_note_whose_crop_the_stream_passed_leaves_the_sum_unremoved() {
    let g = graph_of(
        "cropped",
        &[
            ("pad", PAD),
            ("tone", "crop(sin(2*pi*f0*t), 0s, 0.05s)\n"),
            (
                "blip",
                "crop(lowpass(sample(sin(2*pi*f0*t)), cutoff=2000, q=0.7), 0s, 0.05s)\n",
            ),
        ],
    );
    let stream = opened(&g, "@notes", None);
    let add = |term: &str| {
        added(&stream, &g, &expr(term), &NoStore)
            .now()
            .unwrap_or_else(|e| panic!("`{term}`: {e}"))
    };
    add("@pad(t, f0=200)");
    blocks(&stream, 2);
    let terms = || stream.borrow().counts().terms;
    for node in ["tone", "blip"] {
        let at = stream.borrow().position();
        add(&format!("@{node}(t - {at}sp, f0=300)"));
        assert_eq!(terms(), 2, "{node} sounds");
        blocks(&stream, 1);
        assert_eq!(terms(), 2, "{node} sounds on");
        blocks(&stream, 1);
        assert_eq!(terms(), 1, "{node} ended");
    }
    let at = stream.borrow().position();
    add(&format!("crop(@pad(t, f0=300), 0s, {}sp)", at - 1));
    assert_eq!(terms(), 1, "a note cropped into the past");
}

/// A term placed at its landing has its sample 0 at the sample the add lands at, and a key-up
/// placed there too is written from that same sample 0: the stream is the whole render of the
/// note shifted to where it landed.
#[test]
fn a_term_placed_at_its_landing_plays_from_its_sample_zero_there() {
    let g = graph_of("landing", &[("pad", PAD), ("note", "@pad(t, f0=300)\n")]);
    let stream = opened(&g, "@notes", None);
    let mut heard = blocks(&stream, 5);
    let held = expr("@pad(t, f0=300)");
    let build = |_: &Stream| {
        Ok::<_, sva_engine::EngineError>(Change::Add(g.clone(), held.clone(), Placed::Landing))
    };
    let Ok(Changed::Added(note)) = change(&stream, build, &NoStore).now() else {
        panic!("an add answers its handle");
    };
    let landed = 5 * BLOCK as i64;
    assert_eq!(stream.borrow().landed(note), Some(landed));
    heard.extend(blocks(&stream, 2));
    let up = expr(&format!("@pad(t, f0=300, release={}sp)", 2 * BLOCK));
    let build = |_: &Stream| {
        Ok::<_, sva_engine::EngineError>(Change::Replace(
            note,
            g.clone(),
            up.clone(),
            Placed::Landing,
        ))
    };
    assert_eq!(
        change(&stream, build, &NoStore).now().ok(),
        Some(Changed::Held(true))
    );
    heard.extend(blocks(&stream, 8));

    let alone = whole(&g, "note", 2 * BLOCK);
    assert!(alone.iter().any(|v| *v != 0.0));
    assert_eq!(
        heard[5 * BLOCK..7 * BLOCK],
        alone[..],
        "the attack is whole"
    );
    let mut whole_g = g.clone();
    let last = format!("@pad(t - {landed}sp, f0=300, release={}sp)", 2 * BLOCK);
    assert!(whole_g.define("final", expr(&last)));
    assert_eq!(heard, whole(&whole_g, "final", heard.len()), "{last}");
}

/// Once every note has faded past the stream, none is left in the sum; the echo over it
/// rings on through an edit, and a note added after plays as the whole render of both.
#[test]
fn every_faded_note_leaves_the_sum() {
    let g = composition(1.0);
    let stream = opened(&g, "@echo(t, x=@notes)", None);
    stream.borrow_mut().go_live();
    let a = format!("@blip(t - {BLOCK}sp, f0=200)");
    added(&stream, &g, &expr(&a), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    let mut heard = blocks(&stream, 20);
    assert_eq!(stream.borrow().counts().terms, 0, "the note faded");
    edit(&stream, &g, "@echo(t, x=@notes)");
    heard.extend(blocks(&stream, 4));
    let b = format!("@blip(t - {}sp, f0=300)", 24 * BLOCK);
    added(&stream, &g, &expr(&b), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    heard.extend(blocks(&stream, 20));
    assert_eq!(stream.borrow().counts().terms, 0, "both notes faded");
    assert!(
        stream.borrow().dropped().is_empty(),
        "{:?}",
        stream.borrow().dropped()
    );

    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&format!("@echo(t, x={a} + {b})"))));
    assert_eq!(heard, whole(&whole_g, "final", heard.len()));
}

/// A note added where its master is silent still lands, and leaves the sum only once its own
/// support ended.
#[test]
fn a_note_added_under_a_silent_master_lands() {
    let g = composition(1.0);
    let stream = opened(&g, &format!("crop(@notes, 0s, {BLOCK}sp)"), None);
    blocks(&stream, 2);
    let build = |_: &Stream| {
        let term = expr("@blip(t, f0=300)");
        Ok::<_, sva_engine::EngineError>(Change::Add(g.clone(), term, Placed::Landing))
    };
    let Ok(Changed::Added(note)) = change(&stream, build, &NoStore).now() else {
        panic!("an add answers its handle");
    };
    let at = 2 * BLOCK as i64;
    assert_eq!(stream.borrow().landed(note), Some(at), "the note landed");
    blocks(&stream, 12);
    assert_eq!(stream.borrow().landed(note), Some(at), "the note sounds on");
    blocks(&stream, 1);
    assert_eq!(stream.borrow().counts().terms, 0, "the note ended");
}

/// A term added live reading a stateful value at a moving index reads samples of it that only
/// later instants ask: none is past, so the value is computed whole, bit for bit, and nothing
/// is dropped.
#[test]
fn a_live_term_reading_a_stateful_value_at_a_moving_index_computes_it_whole() {
    let g = graph_of(
        "moving",
        &[(
            "looped",
            "crop(lowpass(sample(saw(110*t)), cutoff=900, q=0.7), 0s, 0.1s)\n",
        )],
    );
    let term = format!(
        "crop(@looped[idx((t - {0}sp) % 800sp)], {0}sp, inf)",
        8 * BLOCK
    );
    let stream = opened(&g, "@notes", None);
    stream.borrow_mut().go_live();
    let mut heard = blocks(&stream, 4);
    added(&stream, &g, &expr(&term), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    heard.extend(blocks(&stream, 16));
    assert!(
        stream.borrow().dropped().is_empty(),
        "{:?}",
        stream.borrow().dropped()
    );
    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(&term)));
    assert_eq!(heard, whole(&whole_g, "final", heard.len()));
}

/// A term added live reading one stateful value at two shifts: the later read reaches only
/// samples of it that later instants ask, so the value starts where the earlier read stands,
/// never where the later one does, and from the add on the stream plays the whole render.
#[test]
fn a_live_term_reading_a_value_twice_starts_it_where_the_earliest_read_stands() {
    let g = composition(1.0);
    let stream = opened(&g, "@notes", None);
    stream.borrow_mut().go_live();
    let mut heard = blocks(&stream, 4);
    let term = "@blip(t - 1000sp, f0=300) + @blip(t - 1500sp, f0=300)";
    added(&stream, &g, &expr(term), &NoStore)
        .now()
        .unwrap_or_else(|e| panic!("{e}"));
    heard.extend(blocks(&stream, 16));
    let mut whole_g = g.clone();
    assert!(whole_g.define("final", expr(term)));
    let whole = whole(&whole_g, "final", heard.len());
    let now = 4 * BLOCK;
    assert!(
        whole[now..].iter().any(|v| *v != 0.0),
        "silence tests nothing"
    );
    assert_eq!(heard[now..], whole[now..]);
}
