// Concern: proves the canonical form is one image per written closed form | Non-concern: typing it (inference.rs) | IO: (a Body) -> the SpectralSum it always lowers to

mod fixtures;

use fixtures::{constant, gaussian, line, part, sine};
use sva_formula::{Body, C64, Edge, Gauss, Singular, Unary, Var, hash_spectral_sum, normalize};

fn atoms(f: &Body) -> Vec<sva_formula::SpectralAtom> {
    normalize(f, Var::T)
        .expect("the fixture stays in A")
        .lanes
        .remove(0)
        .atoms
}

fn imaginary_line(hz: f64, sign: f64) -> Body {
    Body::Apply(
        Unary::Exp,
        part(Body::Mul(vec![
            part(Body::Const(C64::new(
                0.0,
                sign * 2.0 * std::f64::consts::PI * hz,
            ))),
            part(line()),
        ])),
    )
}

#[test]
fn two_spellings_of_a_sinusoid_are_one_normal() {
    let written = normalize(&sine(440.0), Var::T).unwrap();
    let spelled_out = normalize(
        &Body::Div(
            part(Body::Add(vec![
                part(imaginary_line(440.0, 1.0)),
                part(Body::Mul(vec![
                    part(constant(-1.0)),
                    part(imaginary_line(440.0, -1.0)),
                ])),
            ])),
            part(Body::Const(C64::new(0.0, 2.0))),
        ),
        Var::T,
    )
    .unwrap();
    assert_eq!(written, spelled_out);
    assert_eq!(hash_spectral_sum(&written), hash_spectral_sum(&spelled_out));
}

#[test]
fn a_product_of_indicators_intersects() {
    let inner = Body::Crop {
        of: part(constant(1.0)),
        l: Edge::at(1.0),
        r: Edge::at(5.0),
        rise: 0.0,
        fall: 0.0,
    };
    let outer = Body::Crop {
        of: part(inner),
        l: Edge::at(3.0),
        r: Edge::at(9.0),
        rise: 0.0,
        fall: 0.0,
    };
    let [atom] = atoms(&outer)[..] else {
        panic!("a crop of a crop is one atom");
    };
    assert_eq!(atom.ind.unwrap().l, Edge::at(3.0));
    assert_eq!(atom.ind.unwrap().r, Edge::at(5.0));
}

#[test]
fn two_gaussians_multiply_to_one_gaussian() {
    let product = Body::Mul(vec![part(gaussian(1.0, 0.0)), part(gaussian(3.0, 2.0))]);
    let [atom] = atoms(&product)[..] else {
        panic!("two Gaussians are one Gaussian");
    };
    assert_eq!(atom.gauss, Some(Gauss { a: 4.0, mu: 1.5 }));
    let leftover = (-(1.0 * 3.0) * 4.0 / 4.0f64).exp();
    assert!((atom.c.re - leftover).abs() < 1e-12, "{:?}", atom.c);
}

#[test]
fn an_empty_indicator_intersection_leaves_no_atom() {
    let inner = Body::Crop {
        of: part(constant(1.0)),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.0,
        fall: 0.0,
    };
    let outer = Body::Crop {
        of: part(inner),
        l: Edge::at(2.0),
        r: Edge::at(3.0),
        rise: 0.0,
        fall: 0.0,
    };
    assert!(atoms(&outer).is_empty());
}

#[test]
fn a_shouldered_crop_is_seven_indicator_pieces_per_atom() {
    let shouldered = Body::Crop {
        of: part(sine(440.0)),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.01,
        fall: 0.05,
    };
    assert_eq!(
        atoms(&shouldered).len(),
        14,
        "seven pieces per atom of a sinusoid"
    );
    let plateau = atoms(&shouldered)
        .into_iter()
        .filter(|a| {
            a.ind
                == Some(sva_formula::Indicator {
                    l: Edge::at(0.01),
                    r: Edge::at(0.95),
                })
        })
        .count();
    assert_eq!(plateau, 2, "one plateau piece per atom");
}

/// Only the pole written as a principal value has no ordinary value at itself, so the flag
/// must follow that location through the partial fractions rather than the product.
#[test]
fn a_principal_value_keeps_its_flag_past_a_second_pole() {
    let product = Body::Mul(vec![
        part(Body::Pv(part(line()))),
        part(Body::Pow(
            part(Body::Add(vec![part(line()), part(constant(-2.0))])),
            -1,
        )),
    ]);
    let out = atoms(&product);
    assert_eq!(out.len(), 2, "two distinct poles partial-fraction");
    for atom in out {
        let pole = atom.pole.expect("each residue is a pole");
        assert_eq!(
            pole.pv,
            pole.at == C64::ZERO,
            "the principal value stays on the pole it was written on"
        );
    }
}

#[test]
fn like_atoms_merge_and_exact_zeros_fold() {
    let cancelled = Body::Add(vec![
        part(sine(440.0)),
        part(Body::Mul(vec![part(constant(-1.0)), part(sine(440.0))])),
    ]);
    assert!(atoms(&cancelled).is_empty(), "x - x leaves an empty lane");

    let nearly = Body::Add(vec![
        part(sine(440.0)),
        part(Body::Mul(vec![part(constant(1e-300)), part(sine(440.0))])),
    ]);
    assert_eq!(atoms(&nearly).len(), 2, "no epsilon folds a tiny amplitude");
}

#[test]
fn the_sort_key_is_total_over_minus_zero() {
    let negative = Body::Crop {
        of: part(constant(1.0)),
        l: Edge::at(-0.0),
        r: Edge::at(1.0),
        rise: 0.0,
        fall: 0.0,
    };
    let positive = Body::Crop {
        of: part(constant(1.0)),
        l: Edge::at(0.0),
        r: Edge::at(1.0),
        rise: 0.0,
        fall: 0.0,
    };
    let (a, b) = (
        normalize(&negative, Var::T).unwrap(),
        normalize(&positive, Var::T).unwrap(),
    );
    assert_eq!(a, b);
    assert_eq!(hash_spectral_sum(&a), hash_spectral_sum(&b));

    let at_zero = Body::Delta {
        at: part(Body::Add(vec![part(line()), part(constant(0.0))])),
        order: 0,
    };
    let [atom] = atoms(&at_zero)[..] else {
        panic!("one impulse");
    };
    assert_eq!(atom.sing, Singular::Delta { at: 0.0, order: 0 });
}

/// A real or an imaginary value inverts by one correctly rounded division.
#[test]
fn a_value_on_one_axis_inverts_correctly_rounded() {
    use sva_formula::C64;
    for x in [3.0, 7.0, 0.1, 261.63 / 262.0, 1e-3, -2.5, 1e300] {
        assert_eq!(C64::real(x).inv().re.to_bits(), (1.0 / x).to_bits(), "{x}");
        assert_eq!(
            C64::new(0.0, x).inv().im.to_bits(),
            (-1.0 / x).to_bits(),
            "{x}i"
        );
        assert_eq!(
            (C64::real(1.0) / C64::real(x)).re.to_bits(),
            (1.0 / x).to_bits(),
            "1/{x}"
        );
        let by_i = C64::real(1.0) / C64::new(0.0, x);
        assert_eq!(by_i.im.to_bits(), (-1.0 / x).to_bits(), "1/({x}i)");
    }
}
