// Concern: states that memory answers a node through any length of moved nodes | Non-concern: when a value keeps them (render/table/store.rs) | IO: (keeps) -> Known

use std::sync::Arc;

use sva_formula::{Codomain, Hash};
use sva_samples::{Buffer, Extent, Grid, Label};

use super::{Facts, Keep, Known, Memory, Offered, Recording, Stored};

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
        held: Vec::new(),
    }
}

const FACTS: Facts = Facts {
    target: false,
    samples: 100,
};

fn kept(memory: &Memory, key: Hash, offered: Offered) {
    let node = Some((stored(key), offered, FACTS));
    let keep = Keep {
        samples: None,
        label: None,
        slot: None,
        node,
    };
    memory.keep(key, keep, &mut Recording::over(memory));
}

/// Node `i` moves node `i - 1` a sample on, down to a foot holding samples of its own.
fn chain(memory: &Memory, links: u64) {
    let foot = Offered::Held(vec![Arc::new(Buffer::mono(8_000, vec![0.5; 100]))]);
    kept(memory, Hash(0, 0), foot);
    for i in 1..=links {
        let link = Offered::Moves {
            of: Hash(i - 1, 0),
            by: -1,
        };
        kept(memory, Hash(i, 0), link);
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

/// Each link is walked once while its chain holds, never once per link above it.
#[test]
fn each_link_of_a_chain_is_walked_once_however_often_it_is_asked() {
    let memory = Memory::holding(1 << 30);
    chain(&memory, 200);
    let round = memory.begin();
    for i in 0..=200 {
        assert!(matches!(memory.answer(Hash(i, 0), round), Known::Hit(_)));
    }
    let walked = memory.locked().walked.get();
    assert!(walked <= 2 * 200, "{walked} links walked");
}
