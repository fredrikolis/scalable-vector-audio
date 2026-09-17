// Concern: proves a subcommand reads the composition its argument names, and refuses one nothing holds | Non-concern: what a lint or a trace then says | IO: (--in <dir>) -> a directory or a refusal

mod helpers;

use helpers::fixture;
use sva_cli::{CliError, Command, composition, parse_args};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_string()).collect()
}

#[test]
fn a_named_composition_is_read_instead_of_the_directory_the_process_sits_in() {
    let basic = fixture("basic");
    let held = composition(Some(&basic.display().to_string())).expect("a fixture composition");
    assert_eq!(held, basic);
    assert_ne!(
        held,
        sva_core::cwd().expect("a current directory"),
        "the argument decides, not the process"
    );
}

/// The refusal names the path the caller wrote, which no OS error underneath it does.
#[test]
fn a_path_no_composition_sits_at_refuses_as_not_found() {
    let missing = fixture("no-such-composition");
    let refused = composition(Some(&missing.display().to_string())).expect_err("nothing there");
    assert!(matches!(refused, CliError::NotFound(_)), "{refused:?}");
    assert_eq!(refused.code(), "not_found");
    assert_eq!(refused.exit_code(), 24);
    assert!(
        refused.message().contains("no-such-composition"),
        "the refusal names the path: {}",
        refused.message()
    );

    let file = fixture("basic").join("master");
    let refused = composition(Some(&file.display().to_string())).expect_err("a file, not a tree");
    assert_eq!(refused.code(), "validation_error");
}

#[test]
fn every_subcommand_that_reads_a_composition_takes_the_flag_that_names_one() {
    let Ok(Command::Lint { dir, .. }) = parse_args(&argv(&["lint", "--in", "./song1"])) else {
        panic!("lint takes --in");
    };
    assert_eq!(dir.as_deref(), Some("./song1"));

    let Ok(Command::Trace { target, dir }) =
        parse_args(&argv(&["trace", "kick", "--in", "./song1"]))
    else {
        panic!("trace takes --in");
    };
    assert_eq!((target.as_str(), dir.as_deref()), ("kick", Some("./song1")));

    let Ok(Command::Render(args)) = parse_args(&argv(&[
        "render", "--in", "./song1", "master", "--as", "loudness",
    ])) else {
        panic!("render takes --in before its target");
    };
    assert_eq!(args.dir.as_deref(), Some("./song1"));
    assert_eq!(args.target.as_deref(), Some("master"));

    assert!(
        matches!(
            parse_args(&argv(&["lint", "--in"])),
            Err(CliError::Usage(_))
        ),
        "a flag with no path is a usage refusal"
    );
}
