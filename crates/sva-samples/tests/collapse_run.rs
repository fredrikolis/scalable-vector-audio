// Concern: states that a run summed at any instant stays within its own bound of the exact sum | Non-concern: which rows sum a run | IO: (a wave's lines, t) -> error against a double-double sum

use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

use sva_formula::{Body, Bound, C64, IndexId, Mirror, Part, Run, Series, Unary};
use sva_samples::PSYCHOACOUSTIC_V1;
use sva_samples::collapse::run;

const RATE: u32 = 44_100;

fn part(body: Body) -> Part {
    Part::bare(body)
}

fn real(x: f64) -> Part {
    part(Body::Const(C64::real(x)))
}

/// The series `saw`, `square` and `triangle` lower to.
fn wave(name: &str, hz: f64) -> Series {
    let k = IndexId(0);
    let ordinal = || match name {
        "saw" => Body::Index(k),
        _ => Body::Add(vec![
            part(Body::Mul(vec![real(2.0), part(Body::Index(k))])),
            real(-1.0),
        ]),
    };
    let mut angle = vec![
        part(Body::Mul(vec![
            real(TAU),
            part(ordinal()),
            real(hz),
            part(Body::Line),
        ])),
        real(0.0),
    ];
    if name == "triangle" {
        let shifted = Body::Add(vec![part(Body::Index(k)), real(-1.0)]);
        angle.push(part(Body::Mul(vec![real(PI), part(shifted)])));
    }
    let power = if name == "triangle" { 2 } else { 1 };
    Series {
        index: k,
        lo: 1,
        hi: Bound::Infinite,
        term: part(Body::Div(
            part(Body::Apply(Unary::Sin, part(Body::Add(angle)))),
            part(Body::Pow(part(ordinal()), power)),
        )),
    }
}

fn runs(name: &str, hz: f64) -> Vec<Run> {
    let p = PSYCHOACOUSTIC_V1;
    let ceiling = p.ceiling(RATE);
    let found = sva_formula::lines(&wave(name, hz), ceiling, p.floor(ceiling), p.half_lsb());
    Run::of(&found.taken)
}

#[derive(Clone, Copy)]
struct Dd(f64, f64);

fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let v = s - a;
    Dd(s, (a - (s - v)) + (b - v))
}

fn quick(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd(s, b - (s - a))
}

fn add(x: Dd, y: Dd) -> Dd {
    let s = two_sum(x.0, y.0);
    let t = two_sum(x.1, y.1);
    let s = quick(s.0, s.1 + t.0);
    quick(s.0, s.1 + t.1)
}

fn mul(x: Dd, y: Dd) -> Dd {
    let p = x.0 * y.0;
    let e = x.0.mul_add(y.0, -p) + (x.0 * y.1 + x.1 * y.0);
    quick(p, e)
}

fn neg(x: Dd) -> Dd {
    Dd(-x.0, -x.1)
}

fn over(x: Dd, b: f64) -> Dd {
    let q = x.0 / b;
    let r = add(x, neg(mul(Dd(q, 0.0), Dd(b, 0.0))));
    quick(q, r.0 / b)
}

#[derive(Clone, Copy)]
struct Cd(Dd, Dd);

fn cmul(a: Cd, b: Cd) -> Cd {
    Cd(
        add(mul(a.0, b.0), neg(mul(a.1, b.1))),
        add(mul(a.0, b.1), mul(a.1, b.0)),
    )
}

fn cadd(a: Cd, b: Cd) -> Cd {
    Cd(add(a.0, b.0), add(a.1, b.1))
}

fn of(c: C64) -> Cd {
    Cd(Dd(c.re, 0.0), Dd(c.im, 0.0))
}

/// `x*t` in turns, less its nearest whole turn.
fn turns(x: Dd, t: f64) -> Dd {
    let p = mul(x, Dd(t, 0.0));
    let whole = p.0.round();
    add(p, Dd(-whole, 0.0))
}

/// `exp(2*pi*i*f)` for `|f| <= 1/2`: an eighth-turn table and a Taylor series past it.
fn cis(f: Dd) -> Cd {
    let eighth = (f.0 * 8.0).round();
    let g = add(f, Dd(-eighth / 8.0, 0.0));
    let x = mul(Dd(TAU, 2.4492935982947064e-16), g);
    let x2 = mul(x, x);
    let (mut sin, mut cos) = (x, Dd(1.0, 0.0));
    let (mut s_term, mut c_term) = (x, Dd(1.0, 0.0));
    for n in 1..20 {
        let n = n as f64;
        s_term = neg(over(mul(s_term, x2), (2.0 * n) * (2.0 * n + 1.0)));
        c_term = neg(over(mul(c_term, x2), (2.0 * n - 1.0) * (2.0 * n)));
        sin = add(sin, s_term);
        cos = add(cos, c_term);
    }
    let h = Dd(FRAC_1_SQRT_2, -4.833646656726457e-17);
    let (zero, one) = (Dd(0.0, 0.0), Dd(1.0, 0.0));
    let table = match (eighth as i64).rem_euclid(8) {
        0 => Cd(one, zero),
        1 => Cd(h, h),
        2 => Cd(zero, one),
        3 => Cd(neg(h), h),
        4 => Cd(neg(one), zero),
        5 => Cd(neg(h), neg(h)),
        6 => Cd(zero, neg(one)),
        _ => Cd(h, neg(h)),
    };
    cmul(table, Cd(cos, sin))
}

/// The run's lines at their exact frequencies, summed in double-double.
fn exact(r: &Run, t: f64) -> C64 {
    let start = add(
        Dd(r.offset, 0.0),
        mul(Dd(r.step, 0.0), Dd(r.first as f64, 0.0)),
    );
    let ladder = |sign: f64, amps: &[C64]| {
        let rotor = cis(turns(Dd(sign * start.0, sign * start.1), t));
        let z = cis(turns(Dd(sign * r.step, 0.0), t));
        let (mut at, mut sum) = (rotor, of(C64::ZERO));
        for a in amps {
            sum = cadd(sum, cmul(of(*a), at));
            at = cmul(at, z);
        }
        sum
    };
    let mut sum = ladder(1.0, &r.amps);
    if let Some(mirror) = r.mirrored() {
        sum = cadd(sum, ladder(-1.0, &mirror));
    }
    C64::new(sum.0.0 + sum.0.1, sum.1.0 + sum.1.1)
}

const NOTES: [(&str, f64); 3] = [("C2", 65.406), ("C4", 261.626), ("C6", 1_046.502)];

/// Instants spread over `[from, to)` seconds on the grid, the last sample included.
fn instants(from: f64, to: f64) -> Vec<f64> {
    let (a, b) = (
        (from * f64::from(RATE)) as u64,
        (to * f64::from(RATE)) as u64,
    );
    let mut out: Vec<f64> = (0..48u64)
        .map(|i| (a + (i * 7_919_993) % (b - a)) as f64 / f64::from(RATE))
        .collect();
    out.push((b - 1) as f64 / f64::from(RATE));
    out
}

#[test]
fn a_wave_run_stays_within_its_bound_of_the_exact_sum_at_any_hour() {
    for name in ["saw", "square", "triangle"] {
        for (note, hz) in NOTES {
            let held = runs(name, hz);
            assert_eq!(held.len(), 1, "{name} at {note} is one run");
            let r = &held[0];
            assert_eq!(r.mirror, Mirror::Conjugate, "{name} at {note}");
            let bound = run::bound(r);
            assert!(bound < 1e-11, "{name} at {note}: bound {bound:e}");
            for (from, to) in [(0.0, 60.0), (3_540.0, 3_600.0)] {
                let worst = instants(from, to)
                    .into_iter()
                    .map(|t| (run::at(r, t) - exact(r, t)).abs())
                    .fold(0.0, f64::max);
                eprintln!(
                    "{name} {note} {to:>4}s: {} lines, error {worst:.2e}, bound {bound:.2e}",
                    r.len()
                );
                assert!(
                    worst <= bound,
                    "{name} at {note} to {to}s: {worst:e} over {bound:e}"
                );
            }
        }
    }
}

#[test]
fn a_held_mirror_and_a_lone_ladder_stay_within_their_bounds() {
    let held = runs("square", 65.406).remove(0);
    let mut mirror = held.mirrored().expect("a mirror");
    mirror.iter_mut().for_each(|a| *a = a.scale(0.5));
    let held_mirror = Run {
        mirror: Mirror::Held(mirror),
        ..held.clone()
    };
    let lone = Run {
        mirror: Mirror::None,
        ..held
    };
    for (name, r) in [("held", held_mirror), ("lone", lone)] {
        let bound = run::bound(&r);
        for t in instants(3_540.0, 3_600.0) {
            let err = (run::at(&r, t) - exact(&r, t)).abs();
            assert!(err <= bound, "{name} at {t}: {err:e} over {bound:e}");
        }
    }
}

#[test]
fn a_conjugate_mirror_sums_as_its_amplitudes_spelled_out_do() {
    let held = runs("saw", 110.0).remove(0);
    let spelled = Run {
        mirror: Mirror::Held(held.mirrored().expect("a mirror")),
        ..held.clone()
    };
    for t in instants(0.0, 60.0) {
        assert_eq!(
            run::at(&held, t).bits(),
            run::at(&spelled, t).bits(),
            "at {t}"
        );
    }
}

#[test]
fn loose_lines_are_runs_of_one_and_a_skipped_index_breaks_a_ladder() {
    let bare = [
        sva_formula::Line::bare(440.0, C64::real(0.5)),
        sva_formula::Line::bare(-440.0, C64::real(0.5)),
    ];
    let loose = Run::of(&bare);
    assert_eq!(loose.len(), 1);
    assert_eq!(loose[0].len(), 2);
    let mut lines = runs("saw", 1_000.0).remove(0).lines();
    lines.retain(|l| l.rung.is_some_and(|r| r.k != 3));
    let split = Run::of(&lines);
    assert_eq!(split.len(), 2, "1..3 and 4.. on each side, paired");
    assert!(split.iter().all(|r| r.mirror == Mirror::Conjugate));
}
