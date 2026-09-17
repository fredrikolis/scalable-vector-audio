// Concern: a gain-control node's own envelope in dB, before/after an optional input | Non-concern: finding that node (no compressor/limiter type exists) | IO: (gain envelope[, input]) -> GainReduction

use sva_samples::EnvelopeFrame;

use crate::decibels::to_db;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainReductionFrame {
    pub t_secs: f64,
    pub gain_db: f64,
    pub input_db: Option<f64>,
    pub output_db: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GainReduction {
    pub frames: Vec<GainReductionFrame>,
}

/// `input` is read by index against `gain`'s own frames — matching `--frame`s over the same
/// window, exactly as `--as envelope of @gain-node` already lines up by eye today.
pub fn analyze(gain: &[EnvelopeFrame], input: Option<&[EnvelopeFrame]>) -> GainReduction {
    let frames = gain
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let gain_db = to_db(g.rms);
            let input_db = input.and_then(|f| f.get(i)).map(|f| to_db(f.rms));
            GainReductionFrame {
                t_secs: g.t_secs,
                gain_db,
                input_db,
                output_db: input_db.map(|d| d + gain_db),
            }
        })
        .collect();
    GainReduction { frames }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(rms: &[f64]) -> Vec<EnvelopeFrame> {
        rms.iter()
            .enumerate()
            .map(|(i, &r)| EnvelopeFrame {
                t_secs: i as f64 * 0.01,
                rms: r,
                peak: r,
            })
            .collect()
    }

    #[test]
    fn a_half_gain_node_alone_reads_about_minus_6_db() {
        let g = analyze(&envelope(&[0.5, 0.5]), None);
        assert!((g.frames[0].gain_db + 6.0206).abs() < 0.01);
        assert!(g.frames[0].input_db.is_none());
        assert!(g.frames[0].output_db.is_none());
    }

    #[test]
    fn with_an_input_the_output_is_input_plus_gain_in_db() {
        let gain = envelope(&[0.5]);
        let input = envelope(&[1.0]);
        let g = analyze(&gain, Some(&input));
        let f = g.frames[0];
        assert!((f.input_db.unwrap() - 0.0).abs() < 1e-9);
        assert!((f.gain_db + 6.0206).abs() < 0.01);
        assert!((f.output_db.unwrap() - f.gain_db).abs() < 1e-9);
    }

    #[test]
    fn a_shorter_input_leaves_the_tail_frames_without_a_before_after_reading() {
        let gain = envelope(&[0.5, 0.5, 0.5]);
        let input = envelope(&[1.0]);
        let g = analyze(&gain, Some(&input));
        assert!(g.frames[0].input_db.is_some());
        assert!(g.frames[1].input_db.is_none());
        assert!(g.frames[2].output_db.is_none());
    }
}
