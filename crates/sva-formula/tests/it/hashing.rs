// Concern: proves the content address ignores where a closed form was written and names an index by its binder | Non-concern: what a hash keys (sva-engine) | IO: (a ClosedForm) -> its Hash

use crate::fixtures::{line, part, sine};
use sva_formula::{
    Body, Bound, ClosedForm, Hash, IndexId, Lanes, NodeId, Origin, Part, Series, Var,
    hash_written_with,
};

/// A form that reads no ref, as written.
fn written(body: Body) -> Hash {
    let form = ClosedForm {
        var: Var::T,
        body,
        origin: Origin::new(0),
    };
    hash_written(&form)
}

fn hash_written(form: &ClosedForm) -> Hash {
    hash_written_with(form, &mut |id: NodeId| panic!("no ref here, {id:?} read"))
}

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
    assert_eq!(written(here.clone()), written(there));

    let spaced = Body::Mul(vec![part(sine(440.0)), part(sine(3.1))]);
    assert_ne!(
        written(here),
        written(spaced),
        "a different law is a different address"
    );
}

/// One body on two axes is two values.
#[test]
fn a_body_in_f_is_another_address_than_in_t() {
    let on = |var| {
        hash_written(&ClosedForm {
            var,
            body: sine(440.0),
            origin: Origin::new(0),
        })
    };
    assert_ne!(on(Var::T), on(Var::F));
}

/// One mixer, three address spaces kept apart by their rotates.
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

/// `sum(k, 1, 3, k*t*sum(j, 1, 2, j))` with its two indices numbered as a typing drew them.
fn nested(outer: u32, inner: u32, product: [u32; 2]) -> ClosedForm {
    let index = |k: u32| part(Body::Index(IndexId(k)));
    let series = |k: u32, hi: i64, term: Body| {
        Body::Series(Box::new(Series {
            index: IndexId(k),
            lo: 1,
            hi: Bound::Finite(hi),
            term: part(term),
        }))
    };
    let inside = series(
        inner,
        2,
        Body::Mul(vec![index(product[0]), index(product[1])]),
    );
    ClosedForm {
        var: Var::T,
        body: series(
            outer,
            3,
            Body::Mul(vec![index(outer), part(line()), part(inside)]),
        ),
        origin: Origin::new(0),
    }
}

/// An address names each index by the series binding it, whatever number a typing drew.
#[test]
fn an_index_is_hashed_by_the_series_binding_it_never_by_its_number() {
    let first = nested(1, 2, [1, 2]);
    assert_eq!(hash_written(&first), hash_written(&nested(40, 7, [40, 7])));
    assert_ne!(
        hash_written(&first),
        hash_written(&nested(1, 2, [2, 2])),
        "a term reading the inner index twice is another law"
    );
}
