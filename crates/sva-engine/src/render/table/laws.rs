// Concern: proves a render writes the same bits shared or apart, whole or streamed, over random trees of shifts, crops and loops | Non-concern: one node's arithmetic | IO: (a seed) -> three renders

use crate::cache::NoStore;
use crate::render::{Range, RenderConfig, Stream, StreamConfig, render, render_apart};

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
        let Ok(whole) = render(&g, "root", config(), None) else {
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
        let mut stream = now(Stream::open(&g, &at, stream, None, &NoStore)).expect("it streams");
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
