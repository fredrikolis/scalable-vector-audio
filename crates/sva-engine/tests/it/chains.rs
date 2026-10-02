// Concern: proves a chain of nodes each reading the one below several times renders the bits it always did | Non-concern: what any one node computes | IO: (a chain, its depth) -> a digest of the samples

use crate::fixtures::graph_of;
use sva_engine::{RenderConfig, Tier, render};

const RATE: u32 = 8_000;

/// `n0` a tone, each `nk` the body `body` writes over `@n{k-1}(t)`, `nk` the target.
fn chain(base: &str, depth: usize, crop: &str, body: impl Fn(&str) -> String) -> u64 {
    over(base, (depth, crop), body, &format!("@n{depth}(t)"))
}

fn over(base: &str, (depth, crop): (usize, &str), body: impl Fn(&str) -> String, top: &str) -> u64 {
    let mut files = vec![("n0".to_string(), format!("crop({base}, 0s, {crop})\n"))];
    for k in 1..=depth {
        let read = format!("@n{}(t)", k - 1);
        files.push((
            format!("n{k}"),
            format!("crop({}, 0s, {crop})\n", body(&read)),
        ));
    }
    files.push(("top".to_string(), format!("{top}\n")));
    let held: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let g = graph_of("chain", &held);
    let render = render(&g, "top", RenderConfig::at(RATE), &Tier::default())
        .unwrap_or_else(|e| panic!("{top}: {e}"));
    let id = render.id("top").expect("the root");
    let out = render.output(id).expect("a buffer");
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut word = |w: u64| {
        for byte in w.to_le_bytes() {
            digest = (digest ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    };
    let planes = out.slices();
    word(planes.len() as u64);
    for samples in planes {
        assert!(samples.iter().any(|v| *v != 0.0), "silence tests nothing");
        word(samples.len() as u64);
        samples.iter().for_each(|v| word(v.to_bits()));
    }
    digest
}

const TONE: &str = "sin(2*pi*220*t)";

/// Each level reads the one below three times; the bits are those of the tree written out.
#[test]
fn a_sum_reading_its_source_thrice_at_each_of_twelve_levels() {
    let digest = chain(TONE, 12, "1s", |p| format!("{p}*0.5 + {p}*0.3 + {p}*0.2"));
    assert_eq!(digest, 0xec39_7011_2dae_c8b7);
}

#[test]
fn a_sum_with_a_modulated_term_at_each_of_twelve_levels() {
    let digest = chain(TONE, 12, "1s", |p| {
        format!("{p}*sin(2*pi*3*t) + {p}*0.3 + {p}*0.2")
    });
    assert_eq!(digest, 0x975d_1983_4c26_01b5);
}

#[test]
fn a_sum_with_a_max_at_each_of_twelve_levels() {
    let digest = chain(TONE, 12, "1s", |p| {
        format!("max({p}, 0)*0.5 + {p}*0.3 + {p}*0.2")
    });
    assert_eq!(digest, 0x2150_5fe6_e36e_9ea7);
}

#[test]
fn a_sum_with_a_square_at_each_of_eight_levels() {
    let digest = chain("sin(2*pi*t) + 1.5", 8, "2s", |p| {
        format!("0.5*{p} + 0.3*{p}*{p}")
    });
    assert_eq!(digest, 0xbf63_7e7d_2991_24b6);
}

/// An exponential window bounds what it multiplies before it finds where the product is zero.
#[test]
fn an_exponential_window_over_a_sum_twelve_levels_deep() {
    let sum = |p: &str| format!("{p}*0.5 + {p}*0.3 + {p}*0.2");
    let digest = over(TONE, (12, "1s"), sum, "crop(exp(-3*t)*@n12(t), 0s, 1s)");
    assert_eq!(digest, 0xa515_a431_4935_0c71);
}

/// Read once per path, each term would be `3^12` copies of the tone.
#[test]
fn a_loop_over_a_sum_with_a_modulated_term_twelve_levels_deep() {
    let body = |p: &str| format!("{p}*sin(2*pi*3*t) + {p}*0.3 + {p}*0.2");
    let digest = over(TONE, (12, "1s"), body, "@n12(t) + 0.5*self(t - 17ms)");
    assert_eq!(digest, 0x1491_1a68_85d7_8c9e);
}

/// Samples `top` renders, unpruned, over a chain whose `ck` reads `c{k-1}` as `read` writes it.
fn underflowing(depth: usize, read: impl Fn(&str) -> String, top: &str) -> usize {
    let mut files = vec![("c0".to_string(), "sin(2*pi*220*t)\n".to_string())];
    for k in 1..=depth {
        files.push((
            format!("c{k}"),
            format!("{}\n", read(&format!("c{}", k - 1))),
        ));
    }
    files.push(("top".to_string(), format!("{top}\n")));
    let held: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let g = graph_of("underflowing", &held);
    let mut config = RenderConfig::at(RATE);
    config.profile.prune_db = -1e4;
    let render =
        render(&g, "top", config, &Tier::default()).unwrap_or_else(|e| panic!("{top}: {e}"));
    let id = render.id("top").expect("the root");
    render.output(id).expect("a buffer").plane(0).len()
}

/// The product's underflow ends the range alike, to the sample, twenty refs down as three.
#[test]
fn an_exponential_over_a_tone_twenty_refs_down_ends_where_it_underflows() {
    let read = |p: &str| format!("@{p}(t)");
    let shallow = underflowing(20, read, "crop(exp(-20*t)*@c3(t)*exp(-20*t), 0s, inf)");
    let deep = underflowing(20, read, "crop(exp(-20*t)*@c20(t)*exp(-20*t), 0s, inf)");
    assert!(shallow > RATE as usize, "{shallow} samples");
    assert!(shallow.abs_diff(deep) <= 1, "{shallow} against {deep}");
}

/// Forty levels each reading the one below twice are `2^40` paths, bounded once per node.
#[test]
fn an_exponential_over_a_tone_read_twice_a_level_forty_levels_down_ends() {
    let read = |p: &str| format!("@{p}(t) + @{p}(t - 0.1s)");
    let shallow = underflowing(1, read, "crop(exp(-20*t)*@c1(t)*exp(-20*t), 0s, inf)");
    let deep = underflowing(40, read, "crop(exp(-20*t)*@c40(t)*exp(-20*t), 0s, inf)");
    let one = underflowing(1, read, "crop(exp(-20*t)*@c1(t), 0s, inf)");
    assert!(shallow < deep && deep < one, "{shallow}, {deep}, {one}");
}
