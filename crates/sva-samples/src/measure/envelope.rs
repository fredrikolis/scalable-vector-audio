// Concern: traces level over time as successive frames of RMS and peak | Non-concern: frequency content (spectrum.rs), choosing the frame length | IO: (&[f64], sample rate, frame) -> frames

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeFrame {
    pub t_secs: f64,
    pub rms: f64,
    pub peak: f64,
}

pub fn trace(
    samples: &[f64],
    sample_rate: f64,
    start_secs: f64,
    frame_secs: f64,
) -> Vec<EnvelopeFrame> {
    let stride = ((frame_secs * sample_rate).round() as usize).max(1);
    samples
        .chunks(stride)
        .enumerate()
        .map(|(n, chunk)| EnvelopeFrame {
            t_secs: start_secs + (n * stride) as f64 / sample_rate,
            rms: rms(chunk),
            peak: chunk.iter().fold(0f64, |a, &x| a.max(x.abs())),
        })
        .collect()
}

pub fn rms(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&x| x * x).sum();
    (sum_sq / samples.len() as f64).sqrt()
}
