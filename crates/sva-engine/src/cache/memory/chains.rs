// Concern: states that memory answers a node through any length of moved nodes | Non-concern: how a render offers them (render/offer.rs) | IO: (offers) -> Known

use std::sync::Arc;

use sva_formula::{Codomain, Hash};
use sva_samples::{Buffer, Extent, Grid, Label};

use super::{Facts, Known, Memory, Offered, Stored};

fn stored(key: Hash) -> Stored {
    Stored {
        key,
        identity: key,
        label: Label::measured("psychoacoustic-v1", 8_000),
        width: 1,
        codomain: Codomain::Real,
        rate: None,
        grid: Grid::of(8_000),
        support: Extent::new(0, 100),
        priced: 0,
        moved: 0.0,
        readable: true,
        sampled: true,
        cuts: Vec::new(),
        held: Vec::new(),
    }
}

const SETTLED: Facts = Facts {
    slot: None,
    settled: true,
    target: false,
    samples: 100,
};

/// Node `i` moves node `i - 1` a sample on, down to a foot holding samples of its own.
fn chain(memory: &Memory, links: u64) {
    let foot = Offered::Held(vec![Arc::new(Buffer::mono(8_000, vec![0.5; 100]))]);
    memory.offer(stored(Hash(0, 0)), foot, SETTLED);
    for i in 1..=links {
        let link = Offered::Moves {
            of: Hash(i - 1, 0),
            by: -1,
        };
        memory.offer(stored(Hash(i, 0)), link, SETTLED);
    }
}

#[test]
fn a_node_moved_through_any_number_of_links_is_held() {
    let memory = Memory::holding(1 << 30);
    chain(&memory, 200);
    let round = memory.begin();
    let Known::Hit(held) = memory.answer(Hash(200, 0), round) else {
        panic!("two hundred links down, the foot's samples are held");
    };
    assert_eq!(held.extents(), [Extent::new(200, 300)]);
}
