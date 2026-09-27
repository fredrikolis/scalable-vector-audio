// Concern: the directory a subcommand reads its composition from, and a target's refs inside it | Non-concern: loading it (sva-core), where a reading goes | IO: (cwd, target) -> a directory, a target

use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use sva_ast::TokenKind;
use sva_core::{CliError, cwd};

pub fn composition() -> Result<PathBuf, CliError> {
    let dir = cwd()?;
    match std::fs::read_dir(&dir) {
        Ok(_) => Ok(dir),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(CliError::NotFound(format!(
            "no composition at `{}`",
            dir.display()
        ))),
        Err(e) => Err(CliError::Io(format!(
            "could not read the composition at `{}`: {e}",
            dir.display()
        ))),
    }
}

/// `here` while every ref sits under it; else the nearest directory above the first ref
/// outside it that holds `variables/`, else that ref's own. Each ref is rewritten under it.
pub fn located(here: &Path, target: &str) -> Result<(PathBuf, String), CliError> {
    let Ok(tokens) = sva_ast::tokenize(target) else {
        return Ok((here.to_path_buf(), target.to_string()));
    };
    let refs: Vec<(sva_ast::ByteSpan, PathBuf)> = tokens
        .iter()
        .filter_map(|t| match &t.kind {
            TokenKind::Ref(path) => Some((t.span, normal(&here.join(path)))),
            _ => None,
        })
        .collect();
    let root = match refs.iter().find(|(_, file)| !file.starts_with(here)) {
        None => here.to_path_buf(),
        Some((_, file)) => holding(file),
    };
    let mut out = String::with_capacity(target.len());
    let mut at = 0;
    for (span, file) in &refs {
        let inside = file.strip_prefix(&root).map_err(|_| {
            CliError::Usage(format!(
                "`{target}` reads `{}` and `{}`, which no one composition holds; read one \
                 composition's nodes at a time",
                refs[0].1.display(),
                file.display()
            ))
        })?;
        out.push_str(&target[at..span.start]);
        out.push('@');
        out.push_str(&inside.to_string_lossy());
        at = span.end;
    }
    out.push_str(&target[at..]);
    Ok((root, out))
}

fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

fn holding(file: &Path) -> PathBuf {
    let own = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    own.ancestors()
        .find(|dir| dir.join(sva_ast::VARIABLES).is_dir())
        .map_or(own.clone(), Path::to_path_buf)
}
