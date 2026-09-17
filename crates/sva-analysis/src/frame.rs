// Concern: FFTs successive fixed-length frames, one at a time | Non-concern: what a frame's magnitudes mean | IO: (&[f32], rate, frame secs) -> per-frame magnitudes

use sva_samples::{MAX_PINNED_FRAME, magnitudes, pinned_frame};

/// One chunk of exactly the transform's own window yields one frame: the per-frame
/// resolution `spectrum::analyze` discards. A shorter chunk leaves the window's tail on
/// zeros, a cut the magnitudes carry.
pub struct SpectralFrame {
    pub t_secs: f64,
    pub mags: Vec<f64>,
    pub bin_hz: f64,
    /// The window read, in seconds.
    pub span_secs: f64,
    /// Whether the buffer filled that window.
    pub filled: bool,
}

pub fn spectral_frames(
    samples: &[f32],
    sample_rate: f64,
    start_secs: f64,
    frame_secs: f64,
    hop_secs: f64,
) -> Vec<SpectralFrame> {
    let frame_len = pinned_frame(frame_secs, sample_rate).min(MAX_PINNED_FRAME);
    let hop = ((hop_secs * sample_rate).round() as usize).max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < samples.len() {
        let end = (start + frame_len).min(samples.len());
        let frame: Vec<f64> = samples[start..end].iter().map(|&s| f64::from(s)).collect();
        let (mags, bin_hz, ..) = magnitudes(&frame, sample_rate, Some(frame_secs));
        out.push(SpectralFrame {
            t_secs: start_secs + start as f64 / sample_rate,
            mags,
            bin_hz,
            span_secs: frame_len as f64 / sample_rate,
            filled: end - start == frame_len,
        });
        start += hop;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, sr: f64, secs: f64) -> Vec<f32> {
        (0..(secs * sr) as usize)
            .map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / sr).sin() as f32)
            .collect()
    }

    #[test]
    fn a_frame_per_hop_covers_the_whole_buffer() {
        let sr = 8000.0;
        let samples = tone(440.0, sr, 1.0);
        let frames = spectral_frames(&samples, sr, 2.0, 0.02, 0.01);
        assert!(frames.len() > 40, "{} frames", frames.len());
        assert_eq!(frames[0].t_secs, 2.0, "start_secs carries through");
        assert!(
            (frames[1].t_secs - frames[0].t_secs - 0.01).abs() < 1e-9,
            "hop advances t_secs by hop_secs"
        );
    }

    #[test]
    fn a_short_tail_still_reads_one_zero_padded_frame() {
        let sr = 8000.0;
        let samples = vec![1.0f32; 5];
        let frames = spectral_frames(&samples, sr, 0.0, 0.02, 0.02);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].mags.iter().any(|&m| m > 0.0));
    }

    #[test]
    fn no_samples_reads_no_frames() {
        assert!(spectral_frames(&[], 8000.0, 0.0, 0.02, 0.01).is_empty());
    }
}
