// Concern: states what the short-time transform owes a buffer it takes apart and puts back | Non-concern: the FFT itself (src/fft.rs) | IO: (a buffer) -> the same buffer, or a refusal

use sva_samples::stft::{cola_ok, forward, hann_periodic, inverse};
use sva_samples::{Buffer, PSYCHOACOUSTIC_V1, SampleError, Source};

fn chirp(rate: u32, len: usize) -> Buffer {
    Buffer::mono(
        rate,
        (0..len)
            .map(|i| {
                let t = i as f64 / f64::from(rate);
                (std::f64::consts::TAU * (220.0 + 900.0 * t) * t).sin() * 0.7
                    + (std::f64::consts::TAU * 3000.0 * t).cos() * 0.2
            })
            .collect(),
    )
}

/// Sample 0 included: the inverse divides by the window square it actually accumulated, so
/// the leading and trailing edges are not a steady-state assumption.
#[test]
fn istft_after_no_edit_is_bit_exact_under_cola() {
    let x = chirp(44_100, 5_000);
    let frames = forward(&x, 256, 64).expect("64 divides 256 four ways");
    let (y, label) = inverse(&frames, &PSYCHOACOUSTIC_V1);

    assert_eq!(label.source, Source::Exact);
    assert_eq!(y.len(), x.len());
    for i in 0..x.len() {
        let diff = (y.at(0, i) - x.at(0, i)).abs();
        assert!(diff < 1e-12, "sample {i} moved by {diff}");
    }
}

#[test]
fn a_hop_outside_cola_is_refused() {
    let x = chirp(44_100, 1_024);
    assert!(!cola_ok(&hann_periodic(256), 128));
    assert_eq!(
        forward(&x, 256, 128),
        Err(SampleError::HopOutsideCola {
            window: 256,
            hop: 128
        })
    );
    assert_eq!(
        forward(&x, 300, 64),
        Err(SampleError::WindowNotPowerOfTwo { window: 300 })
    );
}

#[test]
fn istft_after_an_edit_is_labeled_measured() {
    let x = chirp(44_100, 2_048);
    let mut frames = forward(&x, 256, 64).expect("a COLA hop");
    let (re, im) = frames.at(0, 4, 9);
    frames.write(0, 4, 9, re * 0.5, im * 0.5);

    let (y, label) = inverse(&frames, &PSYCHOACOUSTIC_V1);
    assert_eq!(label.source, Source::Measured);
    assert!(frames.edited);
    assert!(
        (0..x.len()).any(|i| (y.at(0, i) - x.at(0, i)).abs() > 1e-12),
        "an edited frame set is generally not the transform of the signal it came from"
    );
}

/// A caller branches on `code()`, so two failures that mean different things cannot answer
/// with one code.
#[test]
fn every_sample_error_answers_its_own_code() {
    let held = [
        SampleError::HopOutsideCola { window: 8, hop: 3 },
        SampleError::WindowNotPowerOfTwo { window: 7 },
        SampleError::WidthMismatch { left: 2, right: 3 },
        SampleError::ChannelOutOfRange { k: 4, width: 2 },
        SampleError::GridTooLarge {
            model: "botteldooren",
            nodes: 9,
            ceiling: 4,
        },
    ];
    let mut codes: Vec<&str> = held.iter().map(SampleError::code).collect();
    let before = codes.len();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(before, codes.len(), "one failure, one code: {codes:?}");
    for e in &held {
        assert!(!e.to_string().is_empty(), "{e:?} says what happened");
    }
}
