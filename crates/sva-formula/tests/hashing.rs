// Concern: proves the content address ignores where a closed form was written and follows the table version | Non-concern: what a hash keys (sva-engine) | IO: (a SpectralSum) -> its Hash

mod fixtures;

use fixtures::{part, sine, terms};
use sva_formula::{
    Body, ClosedForm, Lanes, Origin, Part, TABLE_VERSION, Var, hash::hash_closed_form_under,
    hash::hash_spectral_sum_under, hash_closed_form, hash_spectral_sum, normalize,
    normalize_closed_form,
};

#[test]
fn two_spellings_hash_alike_and_origin_does_not_enter() {
    let here = Body::Mul(vec![
        Part::new(Origin::new(1), sine(440.0)),
        Part::new(Origin::new(2), sine(3.0)),
    ]);
    let there = Body::Mul(vec![
        Part::new(Origin::new(900), sine(440.0)),
        Part::new(Origin::new(901), sine(3.0)),
    ]);
    let (a, b) = (
        normalize(&here, Var::T).unwrap(),
        normalize(&there, Var::T).unwrap(),
    );
    assert_eq!(a, b);
    assert_eq!(hash_spectral_sum(&a), hash_spectral_sum(&b));

    let spaced = Body::Mul(vec![part(sine(440.0)), part(sine(3.1))]);
    assert_ne!(
        hash_spectral_sum(&a),
        hash_spectral_sum(&normalize(&spaced, Var::T).unwrap()),
        "a different law is a different address"
    );
}

#[test]
fn a_table_version_bump_changes_every_hash() {
    for (name, subject) in terms() {
        assert_ne!(
            hash_closed_form_under(&subject, TABLE_VERSION),
            hash_closed_form_under(&subject, TABLE_VERSION + 1),
            "{name} keeps its address across a table bump"
        );
        assert_eq!(
            hash_closed_form(&subject),
            hash_closed_form_under(&subject, TABLE_VERSION)
        );

        let Ok(sum) = normalize_closed_form(&subject) else {
            continue;
        };
        assert_ne!(
            hash_spectral_sum_under(&sum, TABLE_VERSION),
            hash_spectral_sum_under(&sum, TABLE_VERSION + 1),
            "{name} keeps its spectral-sum address across a table bump"
        );
    }
}

#[test]
fn the_written_law_and_its_spectral_sum_are_different_addresses() {
    let subject = ClosedForm {
        var: Var::T,
        body: sine(440.0),
        origin: Origin::new(0),
    };
    let sum = normalize_closed_form(&subject).unwrap();
    assert_ne!(hash_closed_form(&subject).0, hash_spectral_sum(&sum).0);
}

/// One mixer, three address spaces: the rotate is what keeps a closed form's address and
/// the buffer key under it from ever landing on one value.
#[test]
fn each_address_space_stays_its_own_under_the_shared_mixer() {
    let word = 0x0123_4567_89ab_cdefu64;
    let of = |mut lanes: Lanes<0>| {
        lanes.word(word);
        lanes.finish()
    };
    let plain = of(Lanes::default());
    let mut staggered = Lanes::<17>::default();
    staggered.word(word);
    let mut further = Lanes::<23>::default();
    further.word(word);

    assert_ne!(plain, staggered.finish(), "17 is not 0");
    assert_ne!(plain, further.finish(), "23 is not 0");
    assert_ne!(staggered.finish(), further.finish(), "17 is not 23");
    assert_eq!(plain, of(Lanes::default()), "and each is a function");
}
