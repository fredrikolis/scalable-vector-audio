// Concern: resolves optional bpm/meter into resolve_bar_spans' seconds_per_bar | Non-concern: what a resolved span feeds into (lib.rs run()) | IO: (&mut Graph) -> () or CliError

use sva_ast::{Expr, Graph, Literal};

use crate::cli_error::CliError;

/// An unparsable numerator is an error, not a guess.
fn beats_per_bar(meter_str: &str) -> Result<f64, CliError> {
    let numerator = meter_str.split('/').next().ok_or_else(|| {
        CliError::BadTempo(format!(
            "meter `{meter_str}` has no `/`-separated numerator"
        ))
    })?;
    numerator.trim().parse::<f64>().map_err(|e| {
        CliError::BadTempo(format!(
            "meter `{meter_str}`'s beats-per-bar component `{numerator}` is not a number: {e}"
        ))
    })
}

/// A `b` literal without a tempo refuses rather than defaulting to some seconds-per-bar.
pub fn refuse_unresolved_bars(graph: &Graph) -> Result<(), CliError> {
    match graph.unresolved_bar_literals().first() {
        None => Ok(()),
        Some(path) => Err(CliError::BadTempo(format!(
            "`{path}` uses a `b` time literal, but this composition declares no bpm/meter \
             (add `variables/bpm` and `variables/meter`)"
        ))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tempo {
    pub seconds_per_bar: f64,
    pub beats_per_bar: f64,
}

/// `None` when the composition declares neither `bpm` nor `meter`.
pub fn resolved(graph: &Graph) -> Result<Option<Tempo>, CliError> {
    match (graph.global("bpm"), graph.global("meter")) {
        (None, None) => Ok(None),
        (Some(Expr::Lit(Literal::Num(bpm))), Some(Expr::Lit(Literal::Str(meter_str)))) => {
            let bpm = *bpm;
            if bpm <= 0.0 {
                return Err(CliError::BadTempo(format!(
                    "bpm must be positive, got {bpm}"
                )));
            }
            let beats_per_bar = beats_per_bar(meter_str)?;
            Ok(Some(Tempo {
                seconds_per_bar: beats_per_bar * 60.0 / bpm,
                beats_per_bar,
            }))
        }
        (Some(_), Some(_)) => Err(CliError::BadTempo(
            "`bpm` must be a numeric literal and `meter` a string literal (e.g. \"4/4\")"
                .to_string(),
        )),
        (Some(_), None) => Err(CliError::BadTempo(
            "`bpm` is present but `meter` is missing; both or neither".to_string(),
        )),
        (None, Some(_)) => Err(CliError::BadTempo(
            "`meter` is present but `bpm` is missing; both or neither".to_string(),
        )),
    }
}

/// Resolves every `Bars` span to `Seconds` from optional `bpm`/`meter`; absent, it's a no-op.
pub fn resolve(graph: &mut Graph) -> Result<(), CliError> {
    match resolved(graph)? {
        None => refuse_unresolved_bars(graph),
        Some(tempo) => {
            graph.resolve_bar_spans(tempo.seconds_per_bar);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_per_bar_parses_the_numerator() {
        assert_eq!(beats_per_bar("4/4").unwrap(), 4.0);
        assert_eq!(beats_per_bar("3/4").unwrap(), 3.0);
    }

    #[test]
    fn beats_per_bar_rejects_a_non_numeric_numerator() {
        assert!(beats_per_bar("four/4").is_err());
    }
}
