// Concern: the observations both front ends ask for, each written as a call with its own arguments | Non-concern: argv (sva-cli's args/), taking the reading (sva-engine) | IO: (text) -> Asked

use std::path::{Path, PathBuf};

use sva_engine::{Arg, Given, READINGS, Representation};

use crate::cli_error::CliError;

pub const RETIRED: [(&str, &str); 2] = [
    ("exact-envelope", "envelope, symbolic on a closed form"),
    ("exact-derivative", "derivative, symbolic on a closed form"),
];

/// One reading written as a call, `spectrum(peaks=8, frame=50ms)=out.json`: its name, its
/// arguments as written, and where it goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub args: Vec<(String, String)>,
    /// No destination is stdout.
    pub dest: Option<PathBuf>,
}

/// A call resolved to the reading it names and the arguments only a front end reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub name: String,
    pub representation: Representation,
    pub dest: Option<PathBuf>,
    /// `bindings(node=@voice)`: the instance whose bindings are read.
    pub node: Option<String>,
    /// `ledger(brief=1)`: only the rows that clipped.
    pub brief: bool,
    /// `ledger(skim=1)`: each row's level alone.
    pub skim: bool,
}

pub fn is_wav(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
}

pub fn retired(name: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|(gone, _)| *gone == name)
        .map(|(_, write)| *write)
}

/// A comma list of calls; a comma inside a call's parentheses separates its arguments.
pub fn calls(text: &str) -> Result<Vec<Call>, CliError> {
    split_outside(text, ',')?
        .into_iter()
        .map(|one| call(one.trim()))
        .collect()
}

/// `name`, `name(k=v, ...)`, either followed by `=path`.
pub fn call(text: &str) -> Result<Call, CliError> {
    let head_end = text.find(['(', '=']).unwrap_or(text.len());
    let name = text[..head_end].trim().to_string();
    if name.is_empty() {
        return Err(unwritten(text, "it names no reading"));
    }
    let mut rest = &text[head_end..];
    let mut args = Vec::new();
    if rest.starts_with('(') {
        let close = closing(rest).ok_or_else(|| unwritten(text, "its `(` is never closed"))?;
        let inner = rest[1..close].trim();
        if !inner.is_empty() {
            for arg in split_outside(inner, ',')? {
                let Some((key, value)) = arg.split_once('=') else {
                    return Err(unwritten(
                        text,
                        &format!("`{}` is no named argument; write `key=value`", arg.trim()),
                    ));
                };
                args.push((key.trim().to_string(), value.trim().to_string()));
            }
        }
        rest = &rest[close + 1..];
    }
    let dest = match rest.trim_start().strip_prefix('=') {
        Some("") => {
            return Err(unwritten(
                text,
                "`=` names no destination; drop it to read the reading under \
                 `data.representations`",
            ));
        }
        Some(path) => Some(PathBuf::from(path)),
        None if rest.trim().is_empty() => None,
        None => {
            return Err(unwritten(
                text,
                &format!("`{}` follows the call", rest.trim()),
            ));
        }
    };
    Ok(Call { name, args, dest })
}

/// Every call resolved. An answer keys each reading by its name, so a name asked twice
/// would print one key twice and a JSON reader would keep only the last; it refuses.
pub fn asked_all(calls: &[Call]) -> Result<Vec<Asked>, CliError> {
    let mut out: Vec<Asked> = Vec::with_capacity(calls.len());
    for call in calls {
        let one = asked(call)?;
        if out.iter().any(|held| held.name == one.name) {
            return Err(CliError::Usage(format!(
                "`{}` is asked more than once; ask each reading once, every argument in \
                 its one call",
                one.name
            )));
        }
        out.push(one);
    }
    Ok(out)
}

/// The reading a call names, each argument read as that reading takes it.
pub fn asked(call: &Call) -> Result<Asked, CliError> {
    let name = call.name.as_str();
    if let Some(write) = retired(name) {
        return Err(CliError::Usage(format!(
            "`{name}` left the language; write `{write}`"
        )));
    }
    let Some(row) = READINGS.iter().find(|row| row.name == name) else {
        let names: Vec<&str> = READINGS.iter().map(|row| row.name).collect();
        return Err(CliError::Usage(format!(
            "unknown representation `{name}`; the representations are {}",
            names.join(", ")
        )));
    };
    let (mut given, mut node, mut brief, mut skim) = (Given::default(), None, false, false);
    for (key, raw) in &call.args {
        let Some(arg) = row.takes.iter().find(|arg| arg.key() == key) else {
            let keys: Vec<&str> = row.takes.iter().map(|arg| arg.key()).collect();
            return Err(CliError::Usage(match keys[..] {
                [] => format!("`{name}` takes no arguments, not `{key}`"),
                _ => format!("`{name}` takes {}, not `{key}`", keys.join(", ")),
            }));
        };
        match arg {
            Arg::Peaks => given.peaks = count(raw, key)?,
            Arg::Depth => given.depth = count(raw, key)?,
            Arg::Frame => given.frame = Some(seconds(raw, key)?),
            Arg::Oversample => given.oversample = factor(raw)?,
            Arg::Brief => brief = switch(raw, key)?,
            Arg::Skim => skim = switch(raw, key)?,
            Arg::Node => node = Some(self::node(raw)?),
        }
    }
    if row.takes.contains(&Arg::Node) && node.is_none() {
        return Err(CliError::Usage(format!(
            "`{name}` reads one instance's {name}; name it, as `{name}(node=@voice)`"
        )));
    }
    Ok(Asked {
        name: call.name.clone(),
        representation: row.representation(&given),
        dest: call.dest.clone(),
        node,
        brief,
        skim,
    })
}

/// `raw` split at each `sep` no parenthesis encloses.
fn split_outside(raw: &str, sep: char) -> Result<Vec<&str>, CliError> {
    let (mut depth, mut from, mut out) = (0i32, 0, Vec::new());
    for (at, c) in raw.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&raw[from..at]);
                from = at + c.len_utf8();
            }
            _ => {}
        }
        if depth < 0 {
            return Err(unwritten(raw, "a `)` closes nothing"));
        }
    }
    out.push(&raw[from..]);
    Ok(out)
}

/// The byte of the `)` that closes the `(` `text` opens with.
fn closing(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

fn unwritten(text: &str, why: &str) -> CliError {
    CliError::Usage(format!(
        "`{text}` is no reading: {why}. write a call, as `spectrum(peaks=8, frame=50ms)`, \
         `=path` after it to write it to a file"
    ))
}

pub fn wav_path(raw: &str) -> Result<PathBuf, CliError> {
    match is_wav(Path::new(raw)) {
        true => Ok(PathBuf::from(raw)),
        false => Err(CliError::Usage(format!(
            "`{raw}` is no `.wav` file; mp3 and every other format are out of scope"
        ))),
    }
}

fn count(raw: &str, key: &str) -> Result<usize, CliError> {
    raw.parse::<usize>()
        .map_err(|_| CliError::Usage(format!("`{key}` needs a whole count, got `{raw}`")))
}

fn factor(raw: &str) -> Result<u32, CliError> {
    match raw.parse::<u32>() {
        Ok(k) if (2..=16).contains(&k) && k.is_power_of_two() => Ok(k),
        _ => Err(CliError::Usage(format!(
            "`oversample` needs a power of two from 2 to 16, got `{raw}`"
        ))),
    }
}

/// A positive time the language spells, `50ms` or `1.5s`; a bare number is seconds.
pub fn seconds(raw: &str, key: &str) -> Result<f64, CliError> {
    let held = match sva_ast::tokenize(raw).as_deref() {
        Ok([one]) => match one.kind {
            sva_ast::TokenKind::Num(v)
            | sva_ast::TokenKind::Time(v, sva_ast::SpanUnit::Seconds) => Some(v),
            _ => None,
        },
        _ => None,
    };
    match held {
        Some(v) if v > 0.0 && v.is_finite() => Ok(v),
        _ => Err(CliError::Usage(format!(
            "`{key}` needs a positive time, as `50ms` or `0.05`, got `{raw}`"
        ))),
    }
}

fn switch(raw: &str, key: &str) -> Result<bool, CliError> {
    match raw {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(CliError::Usage(format!(
            "`{key}` is `1` or `0`, got `{raw}`"
        ))),
    }
}

/// A ref as the language writes one, `@voice`.
fn node(raw: &str) -> Result<String, CliError> {
    match raw.strip_prefix('@') {
        Some(path) if !path.is_empty() => Ok(path.to_string()),
        _ => Err(CliError::Usage(format!(
            "`node` names an instance as a ref, as `@voice`, not `{raw}`"
        ))),
    }
}
