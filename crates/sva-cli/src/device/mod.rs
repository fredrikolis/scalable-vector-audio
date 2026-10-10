// Concern: plays a render's samples on a live device block by block as they are pulled | Non-concern: the player process, picking the destination | IO: blocks -> device bytes, Played or CliError

use std::io::Write;

use sva_core::{CliError, encode};
use sva_engine::Buffer;

use crate::wav::{SampleEncoding, quantize};

#[cfg(target_os = "linux")]
mod aplay;

#[cfg(target_os = "linux")]
pub use aplay::Hearing;

#[cfg(not(target_os = "linux"))]
pub use refused::Hearing;

/// Before anything renders: no platform but Linux has a backend.
pub fn refuse_unsupported(device: &str) -> Result<(), CliError> {
    match cfg!(target_os = "linux") {
        true => Ok(()),
        false => Err(CliError::usage(
            "destination.unsupported_platform",
            format!(
                "`alsa:{device}` plays through ALSA's `aplay`, which only Linux has; write the \
                 samples to a `.wav` path instead"
            ),
        )),
    }
}

/// The ALSA sample format each encoding sends, as `aplay -f` names it.
pub fn format(encoding: SampleEncoding) -> &'static str {
    match encoding {
        SampleEncoding::Float => "FLOAT_LE",
        SampleEncoding::Pcm(bits) if bits <= 8 => "S8",
        SampleEncoding::Pcm(_) => "S16_LE",
    }
}

/// Why a block did not reach the device.
#[derive(Debug)]
pub enum Unwritten {
    /// A sample the format cannot hold, as a `.wav` refuses it.
    Sample(CliError),
    Sink(std::io::Error),
}

/// Interleaves each block into the bytes `format` names and writes them on, whole.
pub struct Writer<W: Write> {
    sink: W,
    encoding: SampleEncoding,
    node: String,
    frames: u64,
}

impl<W: Write> Writer<W> {
    pub fn new(sink: W, encoding: SampleEncoding, node: &str) -> Writer<W> {
        Writer {
            sink,
            encoding,
            node: node.to_string(),
            frames: 0,
        }
    }

    pub fn block(&mut self, block: &Buffer) -> Result<(), Unwritten> {
        let planes: Vec<Vec<f32>> = (0..block.width())
            .map(|c| encode::float32(&self.node, block.plane(c)))
            .collect::<Result<_, _>>()
            .map_err(Unwritten::Sample)?;
        if let SampleEncoding::Pcm(_) = self.encoding {
            planes
                .iter()
                .try_for_each(|plane| encode::full_scale(&self.node, plane))
                .map_err(Unwritten::Sample)?;
        }
        let frames = block.len();
        let mut bytes = Vec::with_capacity(frames * planes.len() * 4);
        for at in 0..frames {
            for plane in &planes {
                let s = plane[at];
                match self.encoding {
                    SampleEncoding::Float => bytes.extend(s.to_le_bytes()),
                    SampleEncoding::Pcm(bits) if bits <= 8 => {
                        bytes.extend((quantize(s, bits, 8) as i8).to_le_bytes());
                    }
                    SampleEncoding::Pcm(bits) => {
                        bytes.extend((quantize(s, bits, 16) as i16).to_le_bytes());
                    }
                }
            }
        }
        self.sink.write_all(&bytes).map_err(Unwritten::Sink)?;
        self.frames += frames as u64;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
}

#[cfg(not(target_os = "linux"))]
mod refused {
    use sva_core::{CliError, Hear, Played};
    use sva_engine::Buffer;

    use crate::wav::SampleEncoding;

    pub enum Hearing {}

    impl Hearing {
        pub fn open(
            device: &str,
            _: f64,
            _: SampleEncoding,
            _: (&str, &str),
        ) -> Result<Hearing, CliError> {
            super::refuse_unsupported(device).map(|()| unreachable!("refused off Linux"))
        }

        pub fn stopped(&self) -> Option<CliError> {
            match *self {}
        }

        pub fn finish(self) -> Result<Played, CliError> {
            match self {}
        }
    }

    impl Hear for Hearing {
        fn heard(&self, _: &Buffer) -> bool {
            match *self {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(left: Vec<f64>, right: Vec<f64>) -> Buffer {
        Buffer::of_planes(48_000, vec![left, right])
    }

    #[test]
    fn a_float_block_is_sent_interleaved_little_endian_frame_by_frame() {
        let mut sent = Vec::new();
        let mut writer = Writer::new(&mut sent, SampleEncoding::Float, "@song");
        let ok = writer.block(&stereo(vec![0.5, -0.25], vec![1.0, 0.0]));
        assert!(ok.is_ok(), "{ok:?}");
        let ok = writer.block(&stereo(vec![0.125], vec![-1.0]));
        assert!(ok.is_ok(), "{ok:?}");
        assert_eq!(writer.frames(), 3);
        let sent: Vec<f32> = sent
            .chunks(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        assert_eq!(sent, [0.5, 1.0, -0.25, 0.0, 0.125, -1.0]);
    }

    #[test]
    fn pcm_is_quantized_as_a_wav_is_in_the_word_its_format_names() {
        let mut sent = Vec::new();
        let mut writer = Writer::new(&mut sent, SampleEncoding::Pcm(16), "@song");
        assert!(writer.block(&stereo(vec![1.0], vec![-0.5])).is_ok());
        assert_eq!(sent, [0xff, 0x7f, 0x00, 0xc0]);
        let mut sent = Vec::new();
        let mut writer = Writer::new(&mut sent, SampleEncoding::Pcm(8), "@song");
        let mono = Buffer::of_planes(8_000, vec![vec![1.0]]);
        assert!(writer.block(&mono).is_ok());
        assert_eq!(sent, [0x7f]);
        assert_eq!(format(SampleEncoding::Pcm(8)), "S8");
        assert_eq!(format(SampleEncoding::Pcm(12)), "S16_LE");
        assert_eq!(format(SampleEncoding::Float), "FLOAT_LE");
    }

    /// Integer PCM holds full scale and nothing past it; nothing of the block is sent.
    #[test]
    fn a_sample_past_full_scale_refuses_before_any_byte_of_its_block_is_sent() {
        let mut sent = Vec::new();
        let mut writer = Writer::new(&mut sent, SampleEncoding::Pcm(16), "@song");
        let refused = writer.block(&stereo(vec![0.5, 1.5], vec![0.0, 0.0]));
        let Err(Unwritten::Sample(e)) = refused else {
            panic!("refused as a sample: {refused:?}");
        };
        assert_eq!(e.diagnostics()[0].code, "render.unrepresentable_sample");
        assert_eq!(writer.frames(), 0);
        assert!(sent.is_empty());
    }

    struct Gone;

    impl Write for Gone {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_sink_that_stopped_taking_bytes_says_so_and_counts_nothing_played() {
        let mut writer = Writer::new(Gone, SampleEncoding::Float, "@song");
        let refused = writer.block(&stereo(vec![0.0], vec![0.0]));
        let Err(Unwritten::Sink(e)) = refused else {
            panic!("refused by the sink: {refused:?}");
        };
        assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(writer.frames(), 0);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn off_linux_a_device_refuses_as_an_unsupported_platform() {
        let Err(CliError::Usage { code, .. }) = refuse_unsupported("default") else {
            panic!("refused");
        };
        assert_eq!(code, "destination.unsupported_platform");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn on_linux_a_device_is_not_refused_for_its_platform() {
        assert!(refuse_unsupported("hw:1").is_ok());
    }
}
