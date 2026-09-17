// Concern: reads/writes components as one interleaved WAV, any bit depth or float | Non-concern: choosing the destination | IO: (planes, sample_rate, path) <-> () or Vec<Vec<f32>>

use std::path::Path;

use sva_core::CliError;

/// `Pcm16` is the format tag Python's stdlib `wave` module can open; hound marks any
/// `bits_per_sample > 16` format `WAVE_FORMAT_EXTENSIBLE`, which it refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleEncoding {
    Float,
    Pcm16,
}

pub fn write_channels(
    planes: &[&[f32]],
    sample_rate: u32,
    path: &Path,
    encoding: SampleEncoding,
) -> Result<(), CliError> {
    match encoding {
        SampleEncoding::Float => write_as::<f32>(planes, sample_rate, path, 32, |s| s),
        SampleEncoding::Pcm16 => write_as::<i16>(planes, sample_rate, path, 16, quantize),
    }
}

pub fn write_wav(
    samples: &[f32],
    sample_rate: u32,
    path: &Path,
    encoding: SampleEncoding,
) -> Result<(), CliError> {
    write_channels(&[samples], sample_rate, path, encoding)
}

fn quantize(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

/// A file that is not there is `not_found`, never the `internal_error` of a tool that broke.
fn unopened(path: &Path, e: &hound::Error) -> CliError {
    match e {
        hound::Error::IoError(io) if io.kind() == std::io::ErrorKind::NotFound => {
            CliError::NotFound(format!("no such file: {}", path.display()))
        }
        e => CliError::Io(format!("could not open {}: {e}", path.display())),
    }
}

/// De-interleaves every channel into its own plane at the file's own native rate — no
/// resampling, ever, so a reference recording's own clock is what every reading is against.
pub fn read_channels(path: &Path) -> Result<(Vec<Vec<f32>>, u32), CliError> {
    let mut reader = hound::WavReader::open(path).map_err(|e| unopened(path, &e))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    if channels == 0 {
        return Err(CliError::Io(format!(
            "{} declares 0 channels",
            path.display()
        )));
    }
    let mut planes: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let bail = |e: hound::Error| CliError::Io(format!("could not read {}: {e}", path.display()));
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for (i, sample) in reader.samples::<f32>().enumerate() {
                planes[i % channels].push(sample.map_err(bail)?);
            }
        }
        hound::SampleFormat::Int => {
            let full_scale = ((1i64 << (spec.bits_per_sample - 1)) - 1) as f32;
            for (i, sample) in reader.samples::<i32>().enumerate() {
                planes[i % channels].push(unquantize(sample.map_err(bail)?, full_scale));
            }
        }
    }
    Ok((planes, spec.sample_rate))
}

pub fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), CliError> {
    let (mut planes, sample_rate) = read_channels(path)?;
    Ok((planes.remove(0), sample_rate))
}

/// The inverse of [`quantize`]: an integer PCM sample, whatever its bit depth, back to f32 in
/// `[-1, 1]` around the same full-scale magnitude it was quantized against.
fn unquantize(sample: i32, full_scale: f32) -> f32 {
    sample as f32 / full_scale
}

fn write_as<S: hound::Sample>(
    planes: &[&[f32]],
    sample_rate: u32,
    path: &Path,
    bits_per_sample: u16,
    convert: impl Fn(f32) -> S,
) -> Result<(), CliError> {
    let sample_format = if bits_per_sample == 32 {
        hound::SampleFormat::Float
    } else {
        hound::SampleFormat::Int
    };
    let spec = hound::WavSpec {
        channels: planes.len().max(1) as u16,
        sample_rate,
        bits_per_sample,
        sample_format,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| CliError::Io(format!("could not create {}: {e}", path.display())))?;
    for i in 0..planes.first().map_or(0, |p| p.len()) {
        for plane in planes {
            writer.write_sample(convert(plane[i])).map_err(|e| {
                CliError::Io(format!("could not write sample to {}: {e}", path.display()))
            })?;
        }
    }
    writer
        .finalize()
        .map_err(|e| CliError::Io(format!("could not finalize {}: {e}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("sva-cli-wav-{name}-{:x}.wav", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn pcm16_writes_a_plain_format_tag_a_bare_riff_parser_can_read() {
        let path = tmp("pcm16-tag");
        write_wav(&[0.5, -0.5, 1.5, -1.5], 44100, &path, SampleEncoding::Pcm16).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        let format_tag = u16::from_le_bytes([bytes[20], bytes[21]]);
        assert_ne!(
            format_tag, 0xFFFE,
            "WAVE_FORMAT_EXTENSIBLE, which Python's `wave` refuses"
        );
        assert_eq!(format_tag, 1, "WAVE_FORMAT_PCM");

        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        let samples: Vec<i16> = reader.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(samples, vec![16384, -16384, i16::MAX, -i16::MAX]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn float_still_carries_every_bit_and_still_reads_back_as_extensible() {
        let path = tmp("float-tag");
        let pcm = [0.125f32, -0.25, 0.75];
        write_wav(&pcm, 8000, &path, SampleEncoding::Float).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let format_tag = u16::from_le_bytes([bytes[20], bytes[21]]);
        assert_eq!(
            format_tag, 0xFFFE,
            "float always needs WAVEFORMATEXTENSIBLE in hound"
        );

        let mut reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().bits_per_sample, 32);
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        assert_eq!(samples, pcm);

        let _ = std::fs::remove_file(&path);
    }

    /// `write_channels`/`read_channels` are inverses at 32-bit float: no quantization step sits
    /// between them, so the round trip is exact, and the file's own rate comes back untouched.
    #[test]
    fn read_channels_round_trips_a_float_wav_exactly_at_its_own_rate() {
        let path = tmp("read-float-roundtrip");
        let left = [0.5f32, -0.25, 0.75, -1.0];
        let right = [-0.5f32, 0.25, -0.75, 1.0];
        write_channels(&[&left, &right], 48_000, &path, SampleEncoding::Float).unwrap();

        let (planes, sample_rate) = read_channels(&path).unwrap();
        assert_eq!(sample_rate, 48_000, "no resampling, ever");
        assert_eq!(planes, vec![left.to_vec(), right.to_vec()]);

        let _ = std::fs::remove_file(&path);
    }

    /// 16-bit PCM quantizes on the way in, so the round trip is within one quantization step,
    /// never exact — the same tolerance `pcm16_writes_a_plain_format_tag...` already measures.
    #[test]
    fn read_channels_unquantizes_pcm16_within_one_step() {
        let path = tmp("read-pcm16-roundtrip");
        let pcm = [0.5f32, -0.5, 0.1, -0.9];
        write_wav(&pcm, 44_100, &path, SampleEncoding::Pcm16).unwrap();

        let (mono, sample_rate) = read_wav(&path).unwrap();
        assert_eq!(sample_rate, 44_100);
        assert_eq!(mono.len(), pcm.len());
        for (got, want) in mono.iter().zip(&pcm) {
            assert!(
                (got - want).abs() < 1.0 / f32::from(i16::MAX),
                "{got} vs {want}"
            );
        }

        let _ = std::fs::remove_file(&path);
    }

    fn write_int(path: &std::path::Path, bits_per_sample: u16, values: &[i32], sample_rate: u32) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for &v in values {
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();
    }

    /// hound's own `Sample for i32` reader already covers 8/16/24/32-bit int — this is the
    /// unquantize half, checked at the two depths `write_channels` never itself produces.
    #[test]
    fn read_channels_unquantizes_every_int_bit_depth_hound_can_decode() {
        let path = tmp("read-8bit");
        write_int(&path, 8, &[127, -128, 0], 22_050);
        let (planes, sample_rate) = read_channels(&path).unwrap();
        assert_eq!(sample_rate, 22_050);
        assert_eq!(planes.len(), 1);
        assert!((planes[0][0] - 1.0).abs() < 0.01, "{:?}", planes[0]);
        assert!((planes[0][1] + 1.0).abs() < 0.01, "{:?}", planes[0]);
        assert_eq!(planes[0][2], 0.0);
        let _ = std::fs::remove_file(&path);

        let path = tmp("read-24bit");
        let full_scale = (1i64 << 23) - 1;
        write_int(
            &path,
            24,
            &[full_scale as i32, -(full_scale as i32), 0],
            96_000,
        );
        let (planes, sample_rate) = read_channels(&path).unwrap();
        assert_eq!(sample_rate, 96_000);
        assert_eq!(planes[0], vec![1.0, -1.0, 0.0]);
        let _ = std::fs::remove_file(&path);
    }

    /// A round-robin de-interleave, checked against a file `write_channels` never produces
    /// itself: an odd number of channels with a visibly different signal on each.
    #[test]
    fn read_channels_de_interleaves_every_component_into_its_own_plane() {
        let path = tmp("read-three-channel");
        let a = [0.1f32, 0.2];
        let b = [0.3f32, 0.4];
        let c = [0.5f32, 0.6];
        write_channels(&[&a, &b, &c], 44_100, &path, SampleEncoding::Float).unwrap();

        let (planes, _) = read_channels(&path).unwrap();
        assert_eq!(planes, vec![a.to_vec(), b.to_vec(), c.to_vec()]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_channels_reports_a_missing_file_as_not_found_not_a_panic() {
        let path = tmp("read-missing");
        assert!(matches!(read_channels(&path), Err(CliError::NotFound(_))));
    }
}
