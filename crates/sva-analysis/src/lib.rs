// Concern: dispatches a name to one post-hoc analysis over already-rendered buffers | Non-concern: rendering them (sva-engine), argv (sva-cli) | IO: (name, Request) -> a JSON fragment or AnalysisError

mod decibels;
pub mod experimental;
mod frame;
mod json;
pub mod stable;

use sva_samples::{EnvelopeFrame, StereoImage};

pub use decibels::to_db;

/// Every name this crate answers `--as` for, once `sva-core::REPRESENTATIONS` has said no.
pub const ANALYSES: [&str; 4] = ["onsets", "trajectory", "masking", "gain-reduction"];

#[derive(Debug, PartialEq)]
pub struct AnalysisError(pub String);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoGrid {
    pub seconds_per_bar: f64,
    pub beats_per_bar: f64,
}

/// Every field an analysis might read; which ones a given `name` actually uses is that
/// analysis's own concern, not this struct's.
pub struct Request<'a> {
    pub samples: &'a [f32],
    pub sample_rate: f64,
    pub start_secs: f64,
    pub frame_secs: f64,
    pub tempo: Option<TempoGrid>,
    pub envelope: &'a [EnvelopeFrame],
    pub stereo: Option<&'a StereoImage>,
    pub against: Option<&'a [f32]>,
    pub gated: bool,
    pub input_envelope: Option<&'a [EnvelopeFrame]>,
}

/// One flat match, no registration: a name past `sva-core::REPRESENTATIONS` lands here.
pub fn run(name: &str, req: &Request) -> Result<String, AnalysisError> {
    match name {
        "onsets" => {
            let o =
                stable::onsets::detect(req.samples, req.sample_rate, req.start_secs, req.tempo)?;
            Ok(json::onsets(&o))
        }
        "trajectory" => {
            let t = stable::trajectory::analyze(
                req.envelope,
                req.stereo,
                req.samples,
                req.sample_rate,
                req.frame_secs,
            );
            Ok(json::trajectory(&t))
        }
        "masking" => {
            let against = req.against.ok_or_else(|| {
                AnalysisError("`masking` needs a second, `--against` node".to_string())
            })?;
            let gate = req
                .gated
                .then(|| {
                    stable::onsets::detect(req.samples, req.sample_rate, req.start_secs, req.tempo)
                })
                .transpose()?;
            experimental::masking::analyze(
                req.samples,
                against,
                req.sample_rate,
                req.start_secs,
                gate.as_ref().map(|o| o.onsets.as_slice()),
            )
            .map(|m| json::masking(&m))
        }
        "gain-reduction" => {
            let g = experimental::gain_reduction::analyze(req.envelope, req.input_envelope);
            Ok(json::gain_reduction(&g))
        }
        other => Err(AnalysisError(format!("unknown representation `{other}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(samples: &[f32]) -> Request<'_> {
        Request {
            samples,
            sample_rate: 44100.0,
            start_secs: 0.0,
            frame_secs: 0.01,
            tempo: None,
            envelope: &[],
            stereo: None,
            against: None,
            gated: false,
            input_envelope: None,
        }
    }

    #[test]
    fn every_listed_analysis_is_reachable_by_name() {
        assert!(run("onsets", &req(&[0.0; 100])).is_ok());
        assert!(run("trajectory", &req(&[0.0; 100])).is_ok());
        assert!(run("gain-reduction", &req(&[])).is_ok());
    }

    #[test]
    fn masking_without_against_refuses_with_a_named_flag() {
        let err = run("masking", &req(&[0.0; 100])).unwrap_err();
        assert!(err.0.contains("--against"));
    }

    #[test]
    fn an_unknown_name_refuses() {
        assert!(run("vibes", &req(&[])).is_err());
    }
}
