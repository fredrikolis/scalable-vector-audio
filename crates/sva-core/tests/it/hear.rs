// Concern: states a render hands its target's samples to a hearer block by block, the same bits it answers, and stops when one is not heard | Non-concern: any device | IO: (a source, a Hear) -> blocks

use std::cell::RefCell;

use sva_core::{CliError, Hear, Job, Output, Session, asked, call, execute_over};
use sva_engine::{Buffer, EngineError, Tier};

fn now<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    match future
        .as_mut()
        .poll(&mut std::task::Context::from_waker(waker))
    {
        std::task::Poll::Ready(out) => out,
        std::task::Poll::Pending => panic!("nothing to wait on over no store"),
    }
}

/// Keeps every block, and hears only as many as `most`.
struct Kept {
    blocks: RefCell<Vec<Buffer>>,
    most: usize,
}

impl Hear for Kept {
    fn heard(&self, block: &Buffer) -> bool {
        let mut blocks = self.blocks.borrow_mut();
        blocks.push(block.clone());
        blocks.len() < self.most
    }
}

fn heard(most: usize) -> (Kept, Result<sva_core::Rendered, CliError>) {
    let held: sva_ast::Composition = [("master", "sample(crop(sin(2*pi*220*t), 0s, 1s))\n")]
        .into_iter()
        .collect();
    let samples = [asked(&call("samples").expect("a call")).expect("a reading")];
    let kept = Kept {
        blocks: RefCell::default(),
        most,
    };
    let job = Job {
        asked: &samples,
        hear: Some(&kept),
        threads: Some(std::num::NonZeroUsize::MIN),
        ..Job::over(&held, "@master")
    };
    let rendered = now(execute_over(job, &Tier::default(), &mut Session::default()));
    (kept, rendered)
}

#[test]
fn every_block_is_heard_as_it_is_pulled_and_together_they_are_the_samples_answered() {
    let (kept, rendered) = heard(usize::MAX);
    let rendered = rendered.unwrap_or_else(|e| panic!("{e}"));
    let answer = rendered
        .answer(&rendered.target, sva_core::Representation::Samples)
        .unwrap_or_else(|e| panic!("{e}"));
    let Output::Samples(whole) = answer.value else {
        panic!("samples answer samples");
    };
    let blocks = kept.blocks.into_inner();
    assert!(blocks.len() > 1, "one second is more than one block");
    let joined: Vec<u64> = blocks
        .iter()
        .flat_map(|b| b.plane(0).iter().map(|v| v.to_bits()))
        .collect();
    let whole: Vec<u64> = whole.plane(0).iter().map(|v| v.to_bits()).collect();
    assert_eq!(joined, whole);
}

#[test]
fn a_block_not_heard_stops_the_render_before_the_next() {
    let (kept, rendered) = heard(1);
    assert!(
        matches!(rendered, Err(CliError::Engine(EngineError::Abandoned))),
        "abandoned"
    );
    assert_eq!(kept.blocks.into_inner().len(), 1);
}
