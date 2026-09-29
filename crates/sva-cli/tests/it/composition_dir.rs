// Concern: proves a target's refs read the current directory, or the composition an absolute ref names | Non-concern: what the render then says | IO: (cwd, target) -> a directory, a target

use crate::helpers::{put, scratch};
use sva_cli::{CliError, located};

#[test]
fn a_relative_ref_reads_the_current_directory() {
    let here = scratch("relative");
    let (root, target) = located(&here, "@voice/note([0, 1s]) * 0.5").expect("a target");
    assert_eq!(root, here);
    assert_eq!(target, "@voice/note([0, 1s]) * 0.5");
}

#[test]
fn an_absolute_ref_reads_the_composition_that_holds_it() {
    let song = scratch("absolute");
    put(&song, "variables/bpm", "120\n");
    put(&song, "voice/note", "sin(2*pi*220*t)\n");
    let elsewhere = scratch("elsewhere");
    let written = format!("@{}([0, 1b], f0=C4)", song.join("voice/note").display());
    let (root, target) = located(&elsewhere, &written).expect("a target");
    assert_eq!(root, song);
    assert_eq!(target, "@voice/note([0, 1b], f0=C4)");
}

#[test]
fn refs_into_two_compositions_refuse() {
    let one = scratch("one");
    let two = scratch("two");
    put(&one, "a", "sin(t)\n");
    put(&two, "b", "sin(t)\n");
    let written = format!(
        "@{} + @{}",
        one.join("a").display(),
        two.join("b").display()
    );
    let refused = located(&scratch("third"), &written).expect_err("two compositions");
    assert!(matches!(refused, CliError::Usage(_)), "{refused:?}");
}
