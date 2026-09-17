// Concern: proves `new` puts a whole composition in place or none of it | Non-concern: what the scaffold holds (src/new.rs's own suite) | IO: (a directory) -> a tree or a refusal

mod helpers;

use std::fs;

use helpers::scratch;

/// A directory where the key belongs fails the last step with every node already written.
#[test]
fn a_scaffold_that_could_not_finish_leaves_no_composition_to_refuse() {
    let dir = scratch("new-partial");
    let key = dir.join(".song1.sva-idempotency-key");
    fs::create_dir(&key).expect("something in the key's place");

    let Err(refused) = sva_cli::scaffold(&dir, "song1", Some("k-1")) else {
        panic!("a key it cannot record is a create that did not finish");
    };
    assert_eq!(refused.exit_code(), 1, "{refused}");
    assert!(!dir.join("song1").exists(), "and nothing is left to refuse");

    fs::remove_dir(&key).expect("the key's place is clear");
    let written = sva_cli::scaffold(&dir, "song1", Some("k-2")).expect("the next create stands");
    assert!(written.root.join("master").is_file());
    assert_eq!(fs::read_to_string(&key).expect("the key"), "k-2");
}
