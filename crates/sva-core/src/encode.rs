// Concern: refuses a sample past what 32-bit floats or integer PCM hold | Non-concern: the file's byte layout (sva-cli wav.rs) | IO: (node, samples) -> f32 samples or CliError

use sva_engine::{Diagnostic, EngineError, Located};

use crate::CliError;

pub fn float32(node: &str, plane: &[f64]) -> Result<Vec<f32>, CliError> {
    plane.iter().map(|&v| float32_sample(node, v)).collect()
}

/// One sample, for a caller filling a buffer it already holds.
pub fn float32_sample(node: &str, v: f64) -> Result<f32, CliError> {
    match v as f32 {
        held if held.is_finite() => Ok(held),
        _ => Err(unrepresentable(
            node,
            v,
            "a 32-bit float",
            "scale the output down",
        )),
    }
}

/// Integer PCM holds full scale and nothing past it.
pub fn full_scale(node: &str, plane: &[f32]) -> Result<(), CliError> {
    match plane.iter().find(|v| v.abs() > 1.0) {
        Some(v) => Err(unrepresentable(
            node,
            f64::from(*v),
            "integer PCM",
            "scale the output into full scale, or write 32-bit floats with --bits 32",
        )),
        None => Ok(()),
    }
}

fn unrepresentable(node: &str, value: f64, format: &str, help: &str) -> CliError {
    CliError::Engine(EngineError::refused(Diagnostic {
        code: "render.unrepresentable_sample".to_string(),
        message: format!("a sample of {value:e} is past what {format} holds"),
        location: Located::at(node, None),
        help: help.to_string(),
    }))
}
