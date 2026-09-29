// Concern: traces level over time as frames of RMS and peak across every channel | Non-concern: frequency content (spectrum.rs), choosing the frame length | IO: (planes, sample rate, frame) -> frames

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeFrame {
    pub t_secs: f64,
    pub rms: f64,
    pub peak: f64,
}

pub fn trace(
    planes: &[&[f64]],
    sample_rate: f64,
    start_secs: f64,
    frame_secs: f64,
) -> Vec<EnvelopeFrame> {
    let stride = ((frame_secs * sample_rate).round() as usize).max(1);
    let len = planes.iter().map(|p| p.len()).min().unwrap_or(0);
    (0..len)
        .step_by(stride)
        .map(|from| {
            let to = (from + stride).min(len);
            let frame: Vec<&[f64]> = planes.iter().map(|p| &p[from..to]).collect();
            EnvelopeFrame {
                t_secs: start_secs + from as f64 / sample_rate,
                rms: level(&frame),
                peak: frame
                    .iter()
                    .flat_map(|p| p.iter())
                    .fold(0f64, |a, &x| a.max(x.abs())),
            }
        })
        .collect()
}

/// One RMS over every channel's samples together.
pub fn level(planes: &[&[f64]]) -> f64 {
    let count: usize = planes.iter().map(|p| p.len()).sum();
    if count == 0 {
        return 0.0;
    }
    let sum_sq: f64 = planes.iter().flat_map(|p| p.iter()).map(|&x| x * x).sum();
    (sum_sq / count as f64).sqrt()
}

pub fn rms(samples: &[f64]) -> f64 {
    level(&[samples])
}
