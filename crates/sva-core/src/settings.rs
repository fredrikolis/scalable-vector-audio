// Concern: parses the settings a render is read under beside its target, condition and readings | Non-concern: where a setting is written (argv, a JS object) | IO: (key, text) -> Settings

use std::path::{Path, PathBuf};

use crate::cli_error::CliError;
use crate::query::{Shaping, is_wav};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    pub flop_budget: Option<u128>,
    /// Seconds a proof looks for the condition where the interval has no end.
    pub proof_limit: Option<f64>,
    /// The instance a reading is taken of, where it is not the target itself.
    pub node: Option<String>,
    pub shape: Shaping,
    pub against: Option<PathBuf>,
    pub brief: bool,
    pub skim: bool,
    pub pcm16: bool,
}

impl Settings {
    /// `raw` as the setting `key` names, refused where `keys` does not list it.
    pub fn set(&mut self, key: &str, raw: &str, keys: &[&str]) -> Result<(), CliError> {
        if !keys.contains(&key) {
            return Err(CliError::Usage(format!(
                "`{key}` is no setting here; the settings are {}",
                keys.join(", ")
            )));
        }
        match key {
            "flop_budget" => self.flop_budget = Some(operations(raw, key)?),
            "proof_limit" => self.proof_limit = Some(seconds(raw, key)?),
            "node" => self.node = Some(raw.to_string()),
            "depth" => self.shape.depth = count(raw, key)?,
            "peaks" => self.shape.peaks = count(raw, key)?,
            "oversample" => self.shape.oversample = factor(raw)?,
            "frame" => self.shape.frame_secs = Some(seconds(raw, key)?),
            "against" => self.against = Some(wav_path(raw)?),
            "brief" => self.brief = switch(raw, key)?,
            "skim" => self.skim = switch(raw, key)?,
            "pcm16" => self.pcm16 = switch(raw, key)?,
            other => {
                return Err(CliError::Usage(format!("`{other}` is no setting")));
            }
        }
        Ok(())
    }
}

pub fn wav_path(raw: &str) -> Result<PathBuf, CliError> {
    match is_wav(Path::new(raw)) {
        true => Ok(PathBuf::from(raw)),
        false => Err(CliError::Usage(format!(
            "`{raw}` is no `.wav` file; mp3 and every other format are out of scope"
        ))),
    }
}

fn operations(raw: &str, key: &str) -> Result<u128, CliError> {
    raw.parse::<u128>().map_err(|_| not_a_count(raw, key))
}

fn count(raw: &str, key: &str) -> Result<usize, CliError> {
    usize::try_from(operations(raw, key)?).map_err(|_| not_a_count(raw, key))
}

fn not_a_count(raw: &str, key: &str) -> CliError {
    CliError::Usage(format!("`{key}` needs a whole count, got `{raw}`"))
}

fn factor(raw: &str) -> Result<u32, CliError> {
    match raw.parse::<u32>() {
        Ok(k) if (2..=16).contains(&k) && k.is_power_of_two() => Ok(k),
        _ => Err(CliError::Usage(format!(
            "`oversample` needs a power of two from 2 to 16, got `{raw}`"
        ))),
    }
}

/// A positive number of seconds, or a time the language spells: `20ms`, `1.5s`.
fn seconds(raw: &str, key: &str) -> Result<f64, CliError> {
    let held = match sva_ast::tokenize(raw).as_deref() {
        Ok([one]) => match one.kind {
            sva_ast::TokenKind::Num(v)
            | sva_ast::TokenKind::Time(v, sva_ast::SpanUnit::Seconds) => Some(v),
            _ => None,
        },
        _ => None,
    };
    match held {
        Some(v) if v > 0.0 => Ok(v),
        _ => Err(CliError::Usage(format!(
            "`{key}` needs a positive number of seconds, as `0.05` or `50ms`, got `{raw}`"
        ))),
    }
}

fn switch(raw: &str, key: &str) -> Result<bool, CliError> {
    match raw {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(CliError::Usage(format!(
            "`{key}` is `true` or `false`, got `{raw}`"
        ))),
    }
}
