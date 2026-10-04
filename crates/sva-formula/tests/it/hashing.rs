// Concern: proves the content address ignores where a closed form was written and names an index by its binder | Non-concern: what a hash keys (sva-engine) | IO: (a ClosedForm) -> its Hash

use crate::fixtures::{line, part, sine};
use sva_formula::{
    Body, Bound, ClosedForm, ContentHasher, Hash, HashDomain, IndexId, NodeId, Origin, Part,
    Series, Var, hash_written_with,
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

/// The address is SipHash-1-3-128 as published, keyed by its domain: these are `siphasher`
/// 0.3.11's `SipHasher13::finish128` outputs under the same keys.
#[test]
fn an_address_is_siphash_1_3_128_keyed_by_its_domain() {
    let mut written = ContentHasher::new(HashDomain::WrittenClosedForm);
    written.word(0x0123_4567_89ab_cdef);
    assert_eq!(
        written.finish(),
        Hash(0x67ba_2470_3361_7d39, 0xd1e5_67c7_88dc_c7e9)
    );
    assert_eq!(
        ContentHasher::new(HashDomain::CacheAddress).finish(),
        Hash(0x2d21_cb22_9972_1eed, 0x13cf_df44_f74a_5fbd)
    );
}

#[test]
fn one_input_in_two_domains_is_two_addresses() {
    let of = |domain| {
        let mut hasher = ContentHasher::new(domain);
        hasher.word(0x0123_4567_89ab_cdef);
        hasher.finish()
    };
    assert_ne!(of(HashDomain::NodeIdentity), of(HashDomain::CacheAddress));
    assert_ne!(of(HashDomain::WrittenClosedForm), of(HashDomain::ReadTime));
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
