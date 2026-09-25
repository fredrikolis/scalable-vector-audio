// Concern: scaffolds a starter composition and the order to read it in | Non-concern: rendering it, or the JSON shape (output.rs) | IO: (dir, name) -> a written tree or CliError

use std::path::{Path, PathBuf};

use sva_core::CliError;

/// `(path under the new directory, contents)`, written in this order so a directory always
/// exists before a file lands in it. Every body stays a closed form until its own layer's
/// written `sample()`, and each node's comment says why it is written that way rather than
/// the way that costs orders of magnitude more.
pub const FILES: [(&str, &str); 11] = [
    (
        "variables/bpm",
        r"; Models: the pulse every `b` literal is measured against | Neglects: the meter, which variables/meter states | IO: () -> beats per minute | Tags: tempo
120
",
    ),
    (
        "variables/meter",
        r"; Models: how many beats a bar holds, so `1b` resolves to seconds | Neglects: the pulse itself, which variables/bpm states | IO: () -> beats per bar | Tags: meter
4/4
",
    ),
    (
        "variables/key",
        r"; Models: the tonic every `st` offset is measured from, so transposing the piece is editing this one number | Neglects: the mode, which chord/home and grid/phrase-2b spell out in their own offsets | IO: () -> hertz | Tags: key
C3
",
    ),
    (
        "voice/tone",
        r"; Models: one voice's timbre, three harmonics rolled off by a lowpass at the third | Neglects: the envelope, which lib/env owns; every parameter carries a default, so `render voice/tone` resolves to one instance instead of refusing as ambiguous | IO: (t, f0, vel) -> amplitude | Tags: voice, harmonics
f0 = @../variables/key
vel = 0.2
lowpass(vel*(sin(2*pi*f0*t) + 0.5*sin(2*pi*2*f0*t) + 0.3333*sin(2*pi*3*f0*t)), 3*f0)
",
    ),
    (
        "lib/env",
        r"; Models: one exponential decay under a shouldered crop, the shape every note wears | Neglects: the timbre it multiplies, which voice/tone owns; written as one window and never as a `min`/`max` pair, which would leave no dual and no exact reading | IO: (t, attack, decay, fade, len) -> a gain | Tags: envelope
attack = 0.005s
decay = 0.25s
fade = 0.05s
len = 0.5s
crop(exp(-t/decay), 0s, len, rise=attack, fall=fade)
",
    ),
    (
        "voice/note",
        r"; Models: one played note, a timbre under an envelope | Neglects: which pitch and when, which chord/home and grid/phrase-2b state | IO: (t, f0, vel, len) -> amplitude | Tags: voice, note
f0 = @../variables/key
vel = 0.2
len = 0.5s
@../voice/tone(t, f0=f0, vel=vel) * @../lib/env(t, len=len)
",
    ),
    (
        "chord/home",
        r"; Models: the tonic triad, a third and a fifth stacked on variables/key as `st` offsets and never as written frequencies, so editing the key transposes the chord | Neglects: the envelope and the rhythm, which voice/note and grid/phrase-2b own | IO: (t) -> amplitude | Tags: chord, triad
@../voice/tone(t, f0=@../variables/key*0st) + @../voice/tone(t, f0=@../variables/key*4st) + @../voice/tone(t, f0=@../variables/key*7st)
",
    ),
    (
        "grid/phrase-2b",
        r"; Models: eight steps over two bars, one note a step, each pitch an `st` offset off variables/key | Neglects: the timbre and the envelope, which voice/tone and lib/env own | IO: (t) -> amplitude | Tags: grid, phrase
@../voice/note(t, f0=@../variables/key*12st)
@../voice/note(t, f0=@../variables/key*7st)
@../voice/note(t, f0=@../variables/key*4st)
@../voice/note(t, f0=@../variables/key*7st)
@../voice/note(t, f0=@../variables/key*9st)
@../voice/note(t, f0=@../variables/key*7st)
@../voice/note(t, f0=@../variables/key*4st)
@../voice/note(t, f0=@../variables/key*0st)
",
    ),
    (
        "perc/hat",
        r"; Models: a closed hat -- one noise band under a shouldered crop, its 0.5 s period commensurate with this 2 s bar, its gain small because a noise's own rms is the square root of half its line count | Neglects: pitch, which no hat has; a `noise(...)*exp(...)` tail would price at 2.3e10 flops against this window's 9.1e7 | IO: (t, gain) -> amplitude | Tags: percussion, noise
gain = 0.0005
crop(gain*noise(1, period=0.5, color=1), 0s, 0.4s, rise=0.002s, fall=0.2s)
",
    ),
    (
        "fx/glue",
        r"; Models: the pitched layers glued by one short feedback -- the pitched chain's one crossing into samples, and the last thing that chain does, because `self` may read only what `sample` has already written | Neglects: the hats, which master sums in beside this under a `sample` of their own | IO: (t) -> amplitude | Tags: fx, feedback
sample(0.5*@../chord/home(t) + @../grid/phrase-2b(t)) + 0.3*self(t - 1sp)
",
    ),
    (
        "master",
        r"; Models: two bars of the whole piece, each layer sampled on its own | Neglects: nothing it does not name; one closed form over the noise and the grid together prices 76x this, and a mono master broadcasts to any width, so no `join` of a value with itself is written | IO: (t) -> amplitude | Tags: master, arrangement
crop(0.6*(@fx/glue(t) + sample(@perc/hat(t)) + sample(@perc/hat(t - 1b))), 0s, 2b)
",
    ),
];

/// `(command, why)`, in the order a stranger should run them: what the composition IS before
/// what it sounds like, and what a render costs before paying for it. Answered beside the
/// tree `new` wrote, so the first thing an agent reads is the next thing it should do.
pub const NEXT: [(&str, &str); 9] = [
    (
        "sva-cli lint",
        "structure, comments, grid rows, key; refuses before any audio",
    ),
    (
        "sva-cli builtins",
        "the whole vocabulary; nothing outside it parses",
    ),
    (
        "sva-cli trace master",
        "what master reads, and why it is samples",
    ),
    (
        "sva-cli render voice/tone --as lines",
        "exact off the closed form, no buffer allocated",
    ),
    (
        "sva-cli render chord/home --as lines",
        "the same triad follows variables/key",
    ),
    (
        "sva-cli render perc/hat --as atoms",
        "a noise is a line series, one atom per line, 2 Hz apart at period=0.5",
    ),
    (
        "sva-cli render master --as flops",
        "what the render costs, counted before it runs",
    ),
    (
        "sva-cli render master --as ledger --skim",
        "per-node rms, peak, clipped",
    ),
    (
        "sva-cli render master --as samples=/tmp/song.wav --sample-rate 48000",
        "the audio itself, at whatever rate you name",
    ),
];

#[derive(Debug)]
pub struct Scaffolded {
    pub root: PathBuf,
    pub files: Vec<String>,
}

/// Refuses if `<dir>/<name>` already exists, so `new` never overwrites a caller's own work.
/// An `idempotency_key` is recorded beside the composition and answers the same `Scaffolded`
/// again when the same key is retried over a tree still holding exactly this scaffold, so a
/// network retry or an agent restart converges rather than erroring.
pub fn scaffold(
    dir: &Path,
    name: &str,
    idempotency_key: Option<&str>,
) -> Result<Scaffolded, CliError> {
    if name.is_empty() || name.contains('/') || name.contains(std::path::MAIN_SEPARATOR) {
        return Err(CliError::Usage(format!(
            "`{name}` must be a single directory name, not a path"
        )));
    }
    let root = dir.join(name);
    if root.exists() {
        let key = idempotency_key.ok_or_else(|| CliError::Conflict {
            by: "new",
            message: format!(
                "{} already exists; `new` never overwrites a composition. State \
                 `--idempotency-key <key>` to have a retry of your own create succeed instead",
                root.display()
            ),
        })?;
        let taken = || CliError::Conflict {
            by: "new",
            message: format!(
                "{} already exists and is not what `--idempotency-key {key}` scaffolded; \
                 `new` never overwrites a composition",
                root.display()
            ),
        };
        if std::fs::read_to_string(key_path(dir, name)).ok().as_deref() != Some(key) {
            return Err(taken());
        }
        if !holds_scaffold(&root) {
            return Err(taken());
        }
        return Ok(Scaffolded {
            root,
            files: FILES.iter().map(|(path, _)| path.to_string()).collect(),
        });
    }
    // Written aside, renamed in, the key last: a create that stopped leaves nothing behind.
    let staging = dir.join(format!(".{name}.sva-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let files = staged(&staging).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
    })?;
    std::fs::rename(&staging, &root).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        CliError::Io(format!("could not put {} in place: {e}", root.display()))
    })?;
    if let Some(key) = idempotency_key {
        std::fs::write(key_path(dir, name), key).map_err(|e| {
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_file(key_path(dir, name));
            CliError::Io(format!(
                "could not record the idempotency key beside {}: {e}",
                root.display()
            ))
        })?;
    }
    Ok(Scaffolded { root, files })
}

fn staged(staging: &Path) -> Result<Vec<String>, CliError> {
    let mut files = Vec::with_capacity(FILES.len());
    for (path, contents) in FILES {
        let full = staging.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::Io(format!("could not create {}: {e}", parent.display())))?;
        }
        std::fs::write(&full, contents)
            .map_err(|e| CliError::Io(format!("could not write {}: {e}", full.display())))?;
        files.push(path.to_string());
    }
    Ok(files)
}

/// Beside the composition, never inside it: every file under a composition root is a node.
fn key_path(dir: &Path, name: &str) -> std::path::PathBuf {
    dir.join(format!(".{name}.sva-idempotency-key"))
}

/// Byte-for-byte, so a retry only succeeds over a tree nothing has edited since.
fn holds_scaffold(root: &Path) -> bool {
    FILES.iter().all(|(path, contents)| {
        std::fs::read_to_string(root.join(path)).is_ok_and(|held| held == *contents)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sva-cli-new-{name}-{:x}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scaffold_writes_every_file_and_reports_it() {
        let dir = tmp("basic");
        let scaffolded = scaffold(&dir, "song1", None).unwrap();
        assert_eq!(scaffolded.root, dir.join("song1"));
        assert_eq!(scaffolded.files.len(), FILES.len());
        for (path, contents) in FILES {
            let on_disk = std::fs::read_to_string(scaffolded.root.join(path)).unwrap();
            assert_eq!(on_disk, contents);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scaffold_refuses_a_name_that_already_exists() {
        let dir = tmp("exists");
        std::fs::create_dir_all(dir.join("song1")).unwrap();
        let err = scaffold(&dir, "song1", None).unwrap_err();
        assert!(matches!(err, CliError::Conflict { .. }));
        assert_eq!(
            err.exit_code(),
            4,
            "a taken name is a conflict, not a fault"
        );
        assert!(err.message().contains("already exists"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `cli` standard's own requirement for a create verb: a retry succeeds identically
    /// rather than erroring, so an agent restart converges instead of dead-ending.
    #[test]
    fn a_retry_under_the_same_idempotency_key_answers_the_first_call_again() {
        let dir = tmp("idempotent");
        let first = scaffold(&dir, "song1", Some("k-1")).expect("a fresh scaffold");
        let again = scaffold(&dir, "song1", Some("k-1")).expect("a retry over the same tree");
        assert_eq!(first.root, again.root);
        assert_eq!(first.files, again.files);

        std::fs::write(first.root.join("master"), "0.0\n").unwrap();
        let other = scaffold(&dir, "song1", Some("k-2")).unwrap_err();
        assert!(
            matches!(other, CliError::Conflict { .. }),
            "someone else's key is not a retry of this create"
        );

        std::fs::write(first.root.join("master"), "0.0\n").unwrap();
        let edited = scaffold(&dir, "song1", Some("k-1")).unwrap_err();
        assert!(
            matches!(edited, CliError::Conflict { .. }),
            "an edited tree is not a retry"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scaffold_refuses_a_name_that_is_a_path() {
        let dir = tmp("pathlike");
        assert!(matches!(
            scaffold(&dir, "a/b", None).unwrap_err(),
            CliError::Usage(_)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A caller's very first `sva-cli lint` after `new` must not immediately refuse them —
    /// the scaffold is the canonical example of the doc-comment convention, not an exception.
    /// `missing-comment`/`multiline-comment`/`malformed-comment` are hard errors now, so a
    /// violation here would surface as `Err`, not as a `Finding` to filter out.
    #[test]
    fn the_scaffolded_composition_lints_clean_of_every_doc_comment_check() {
        let dir = tmp("lints-clean");
        let scaffolded = scaffold(&dir, "song1", None).unwrap();
        let report = crate::lint::lint(&scaffolded.root, None);
        assert!(
            report.is_ok(),
            "a freshly scaffolded composition must carry a well-formed doc comment on every \
             node: {}",
            report.err().map(|e| e.message()).unwrap_or_default()
        );
        assert!(
            report.is_ok_and(|held| held.findings.is_empty()),
            "and no advice either: the quickstart's very first command must come back silent"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `next` line an argv change broke would send a stranger's first move into a refusal.
    #[test]
    fn every_next_command_this_build_still_parses() {
        for (command, why) in NEXT {
            let argv: Vec<String> = command
                .split_whitespace()
                .skip(1)
                .map(str::to_string)
                .collect();
            assert!(
                crate::parse_args(&argv).is_ok(),
                "`new` answers `{command}`, which this build refuses"
            );
            assert!(!why.is_empty(), "`{command}` is answered with no reason");
        }
    }

    /// Nothing the house style refuses outright may be what a first composition teaches.
    #[test]
    fn no_scaffold_body_writes_what_this_engine_reads_expensively() {
        for (path, contents) in FILES {
            let body: String = contents
                .lines()
                .filter(|line| !line.trim_start().starts_with(';'))
                .collect::<Vec<_>>()
                .join("\n");
            for banned in [
                "44100", "48000", "96000", "rand(", "sat(", "tanh(", "min(", "max(", "sum(",
                "join(",
            ] {
                assert!(
                    !body.contains(banned),
                    "`{path}` writes `{banned}`, which the scaffold teaches against"
                );
            }
        }
    }
}
