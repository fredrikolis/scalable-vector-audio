// Concern: proves a render is the same bits shared or apart, whole or in blocks, and a changed stream builds, plays as anew, on random trees | Non-concern: a node's arithmetic | IO: (a seed) -> renders

use std::cell::RefCell;

use crate::cache::Tier;
use crate::render::{
    Change, Changed, Handle, Placed, Range, RenderConfig, Stream, StreamConfig, change, render,
    render_apart,
};

/// Nothing to wait on over no store: one poll finishes it.
fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(waker))
    {
        std::task::Poll::Ready(out) => out,
        std::task::Poll::Pending => panic!("a stream over no store never waits"),
    }
}

const RATE: u32 = 8_000;
const LEN: i64 = 2_400;

/// xorshift64: the same trees on every machine.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, from: &'a [String]) -> &'a str {
        &from[self.below(from.len() as u64) as usize]
    }

    /// A shift in seconds that lands between samples, or a whole count of them.
    fn shift(&mut self) -> String {
        match self.below(2) {
            0 => format!("{}sp", self.below(900)),
            _ => format!("{:.7}s", self.below(1_000_000) as f64 / 9_000_000.0),
        }
    }
}

fn tree(seed: u64) -> Vec<(String, String)> {
    let mut draw = Draw(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut files = Vec::new();
    let (mut forms, mut sampled) = (Vec::new(), Vec::new());
    for i in 0..2 + draw.below(2) {
        let (hz, tau, len) = (
            110 + draw.below(770),
            5 + draw.below(25),
            5 + draw.below(20),
        );
        files.push((
            format!("f{i}"),
            format!("crop(sin(2*pi*{hz}*t)*exp(0 - t/0.{tau:02}), 0s, 0.{len:02}s)\n"),
        ));
        forms.push(format!("f{i}"));
    }
    for i in 0..1 + draw.below(2) {
        let (leaf, cutoff) = (draw.pick(&forms).to_string(), 300 + draw.below(2_000));
        files.push((
            format!("s{i}"),
            format!("lowpass(sample(@{leaf}), cutoff={cutoff}, q=0.7)\n"),
        ));
        sampled.push(format!("s{i}"));
    }
    if draw.below(2) == 0 {
        let (under, back) = (draw.pick(&sampled).to_string(), 3 + draw.below(40));
        files.push((
            "l0".to_string(),
            format!("@{under} + 0.5*self[idx(t) - {back}]\n"),
        ));
        sampled.push("l0".to_string());
    }
    let mut reads: Vec<String> = forms.iter().chain(&sampled).cloned().collect();
    for i in 0..2 + draw.below(2) {
        let terms: Vec<String> = (0..2 + draw.below(2))
            .map(|_| {
                let x = draw.pick(&reads).to_string();
                match draw.below(4) {
                    0 => format!("@{x}(t - {})", draw.shift()),
                    1 => format!(
                        "crop(@{x}(t - {}), 0.0{}s, 0.{}s)",
                        draw.shift(),
                        draw.below(9),
                        1 + draw.below(9)
                    ),
                    2 => format!("0.{}*@{x}", 1 + draw.below(9)),
                    _ => format!("@{x}"),
                }
            })
            .collect();
        files.push((format!("m{i}"), format!("{}\n", terms.join(" + "))));
        reads.push(format!("m{i}"));
    }
    let last = reads.len() - 1;
    files.push((
        "root".to_string(),
        format!(
            "@{}(t - {}) + @{}\n",
            reads[last],
            draw.shift(),
            reads[last - 1]
        ),
    ));
    files
}

fn config() -> RenderConfig {
    RenderConfig {
        range: Range {
            start: Some(0),
            end: Some(LEN),
        },
        ..RenderConfig::at(RATE)
    }
}

fn bits(samples: &[f64]) -> Vec<u64> {
    samples.iter().map(|v| v.to_bits()).collect()
}

/// Random trees of whole and fractional shifts, crops, loops and filters: each value is one
/// function of its own sample index, so sharing it, holding it apart for every read, and
/// cutting it into blocks of any size all write the same bits.
#[test]
fn a_tree_is_the_same_bits_shared_or_apart_whole_or_streamed() {
    let mut rendered = 0;
    for seed in 1..=24u64 {
        let files = tree(seed);
        let mut composition = sva_ast::Composition::new();
        for (name, body) in &files {
            composition.insert(name, body);
        }
        let g = sva_ast::load(&composition).expect("a composition");
        let Ok(whole) = render(&g, "root", config(), &Tier::default()) else {
            continue;
        };
        rendered += 1;
        let want = whole
            .output(whole.root)
            .expect("the root")
            .plane(0)
            .to_vec();
        assert!(
            want.iter().any(|v| *v != 0.0),
            "{seed}: silence tests nothing"
        );
        let apart = render_apart(&g, "root", config()).expect("apart renders");
        let apart = apart
            .output(apart.root)
            .expect("the root")
            .plane(0)
            .to_vec();
        assert_eq!(
            bits(&apart),
            bits(&want),
            "{seed}: shared or apart {files:?}"
        );
        let block = 1 + (seed as usize * 97) % 700;
        let stream = StreamConfig {
            block,
            channels: None,
            render: config(),
        };
        let at = sva_ast::parse_expr("@root").expect("a ref");
        let tier = Tier::default();
        let mut stream = now(Stream::open(&g, &at, stream, &tier)).expect("it streams");
        let mut heard = Vec::new();
        while let Some(block) = stream.read(stream.position(), block).expect("a block") {
            heard.extend_from_slice(block.plane(0));
        }
        assert_eq!(
            bits(&heard),
            bits(&want),
            "{seed}: whole or in blocks of {block}"
        );
    }
    assert!(rendered >= 20, "only {rendered} of 24 trees rendered");
}

/// A value held apart for its read is still named by what it computes: the apart build holds
/// one value per read, several under one key, and only keys the shared build holds.
#[test]
fn a_value_held_apart_is_keyed_as_the_shared_one() {
    let (mut checked, mut repeated) = (0, 0);
    for seed in 1..=12u64 {
        let mut composition = sva_ast::Composition::new();
        for (name, body) in &tree(seed) {
            composition.insert(name, body);
        }
        let g = sva_ast::load(&composition).expect("a composition");
        let (Ok(shared), Ok(apart)) = (
            render(&g, "root", config(), &Tier::default()),
            render_apart(&g, "root", config()),
        ) else {
            continue;
        };
        let keys = |held: &crate::render::Render| -> Vec<_> {
            let table = held.table.as_ref().expect("a table");
            table.values.iter().map(|(_, v)| v.key).collect()
        };
        let (shared, apart) = (keys(&shared), keys(&apart));
        let distinct: std::collections::BTreeSet<_> = apart.iter().copied().collect();
        assert!(!apart.is_empty(), "{seed}");
        assert!(distinct.iter().all(|k| shared.contains(k)), "{seed}");
        repeated += usize::from(distinct.len() < apart.len());
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} trees rendered");
    assert!(repeated > 0, "some tree reads one value twice, held apart");
}

fn expr(text: &str) -> sva_ast::Expr {
    sva_ast::parse_expr(text).expect("an expression")
}

/// One random change: an add, a replace or a remove of a term, an edit of the target, or,
/// `rewrites`, one of a file it plays.
fn changed(
    (stream, tier): (&RefCell<Stream>, &Tier),
    (g, rewrites): (&sva_ast::Graph, bool),
    draw: &mut Draw,
    held: &mut Vec<Handle>,
) {
    let reads: Vec<String> = g.paths().map(str::to_string).collect();
    let at = stream.borrow().position();
    let term = format!(
        "@{}(t - {}sp)",
        draw.pick(&reads),
        at + draw.below(300) as i64
    );
    let placed = match draw.below(2) {
        0 => Placed::Written,
        _ => Placed::Landing,
    };
    let edit = match (draw.below(5), held.len()) {
        (0, _) => Change::Target(
            g.clone(),
            expr(&format!("@notes + 0.{}*@root", 1 + draw.below(9))),
        ),
        (1, n) if n > 0 => Change::Replace(
            held[draw.below(n as u64) as usize],
            g.clone(),
            expr(&term),
            placed,
        ),
        (2, n) if n > 0 => Change::Remove(held.remove(draw.below(n as u64) as usize)),
        (3, _) if rewrites => {
            let (mut edited, path) = (g.clone(), draw.pick(&reads).to_string());
            let body = g.expr(&path).expect("a node").clone();
            let half = sva_ast::Expr::Lit(sva_ast::Literal::Num(0.5));
            let scaled = sva_ast::Expr::Bin(sva_ast::BinOp::Mul, Box::new(half), Box::new(body));
            edited.set(&path, Some(edited.defining(scaled)));
            Change::Target(edited, expr("@notes + @root"))
        }
        _ => Change::Add(g.clone(), expr(&term), placed),
    };
    let mut edit = Some(edit);
    let build = |_: &Stream| Ok::<_, crate::EngineError>(edit.take().expect("built once"));
    if let Ok(Changed::Added(handle)) = now(change(stream, build, tier)) {
        held.push(handle);
    }
}

/// However its terms and target changed, a stream holds the typing and table a build of all it
/// plays, carrying nothing over, would: what a change carries over is what it would build. A
/// memory keeping nothing leaves both answered by no store.
#[test]
fn a_changed_stream_holds_what_a_build_of_all_it_plays_would() {
    let mut checked = 0;
    for seed in 1..=12u64 {
        let mut composition = sva_ast::Composition::new();
        for (name, body) in &tree(seed) {
            composition.insert(name, body);
        }
        let g = sva_ast::load(&composition).expect("a composition");
        let config = StreamConfig {
            block: 100,
            channels: None,
            render: RenderConfig::at(RATE),
        };
        let tier = Tier::new(0);
        let Ok(stream) = now(Stream::open(&g, &expr("@notes + @root"), config, &tier)) else {
            continue;
        };
        let (stream, mut draw, mut held) = (RefCell::new(stream), Draw(seed | 1), Vec::new());
        if seed % 2 == 0 {
            stream.borrow_mut().go_live();
        }
        for _ in 0..16 {
            changed((&stream, &tier), (&g, true), &mut draw, &mut held);
            for _ in 0..draw.below(4) {
                let at = stream.borrow().position();
                stream.borrow_mut().read(at, 100).expect("a block");
            }
            if let Some(unlike) = now(stream.borrow().unlike_rebuilt()) {
                assert_eq!(unlike, Vec::<String>::new(), "{seed}");
                checked += 1;
            }
        }
    }
    assert!(checked >= 150, "only {checked} changes checked");
}

/// After a change that moves nothing before where an exact stream stands, it plays from there
/// what a whole render of all it now plays writes. A rewritten file moves its past too, and a
/// stateful value then carries its state on (edit.rs), so those changes are left out.
#[test]
fn a_changed_stream_plays_from_where_it_stands_what_a_render_of_it_writes() {
    let (mut checked, mut sounded) = (0, 0);
    for seed in 1..=12u64 {
        let mut composition = sva_ast::Composition::new();
        for (name, body) in &tree(seed) {
            composition.insert(name, body);
        }
        let g = sva_ast::load(&composition).expect("a composition");
        let config = StreamConfig {
            block: 100,
            channels: None,
            render: RenderConfig::at(RATE),
        };
        let tier = Tier::default();
        let Ok(stream) = now(Stream::open(&g, &expr("@notes + @root"), config, &tier)) else {
            continue;
        };
        let (stream, mut draw, mut held) = (RefCell::new(stream), Draw(seed | 1), Vec::new());
        for k in 0..16 {
            changed((&stream, &tier), (&g, false), &mut draw, &mut held);
            let at = stream.borrow().position();
            let graph = stream.borrow().graph().clone();
            let ahead = RenderConfig {
                range: Range {
                    start: Some(0),
                    end: Some(at + 300),
                },
                ..RenderConfig::at(RATE)
            };
            let whole = render(&graph, crate::render::STREAMED, ahead, &Tier::default())
                .unwrap_or_else(|e| panic!("{seed}/{k}: what the stream plays renders: {e}"));
            let want = whole
                .output(whole.root)
                .expect("the root")
                .plane(0)
                .to_vec();
            let mut heard = Vec::new();
            while heard.len() < 300 {
                let at = stream.borrow().position();
                match stream.borrow_mut().read(at, 100).expect("a block") {
                    Some(block) => heard.extend_from_slice(block.plane(0)),
                    None => break,
                }
            }
            let from = usize::try_from(at).expect("a position");
            let (played, past) = want[from..from + 300].split_at(heard.len());
            assert_eq!(bits(&heard), bits(played), "{seed}/{k} at {at}");
            assert!(past.iter().all(|v| *v == 0.0), "{seed}/{k}: ended early");
            sounded += usize::from(heard.iter().any(|v| *v != 0.0));
            checked += 1;
        }
    }
    assert!(checked >= 150, "only {checked} changes checked");
    assert!(sounded > checked / 2, "only {sounded} of {checked} sounded");
}
