// Concern: pipes each block to `aplay` as raw PCM, reads back its buffer, underruns and failures | Non-concern: the byte layout (mod.rs) | IO: blocks -> aplay; its stderr -> Played or CliError

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use sva_core::{CliError, Hear, Played};
use sva_engine::Buffer;

use super::{Unwritten, Writer, format};
use crate::wav::SampleEncoding;

/// `aplay` prints this for every xrun it recovers from on the PCM it opened.
const UNDERRUN: &str = "underrun!!!";
/// `aplay` plays on at the rate the device gave, so a mismatch refuses rather than plays wrong.
const INEXACT_RATE: &str = "rate is not accurate";
/// The blocks a render may compute past what aplay has taken, so it neither waits on each
/// block nor holds the whole stream.
const AHEAD: usize = 4;
const SEEN_BY: &str = "aplay, at the ALSA PCM it opened; a sound server behind that PCM, as \
                       `default` often is, reports none of its own";

/// One live device, opened at the first block, when its channels and rate are known.
pub struct Hearing {
    device: String,
    buffer_secs: f64,
    encoding: SampleEncoding,
    representation: String,
    node: String,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    playing: Option<Aplay>,
    stopped: Option<CliError>,
}

struct Aplay {
    child: Child,
    writer: Option<Writer<Queued>>,
    feeder: Option<JoinHandle<()>>,
    said: Arc<Mutex<Vec<String>>>,
    reader: Option<JoinHandle<()>>,
    rate: u32,
    channels: usize,
}

impl Hearing {
    /// Refuses a missing `aplay` here, before anything renders.
    pub fn open(
        device: &str,
        buffer_secs: f64,
        encoding: SampleEncoding,
        (representation, node): (&str, &str),
    ) -> Result<Hearing, CliError> {
        let found = Command::new("aplay")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match found {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(no_aplay(device)),
            Err(e) => return Err(CliError::Io(format!("could not run `aplay`: {e}"))),
        }
        Ok(Hearing {
            device: device.to_string(),
            buffer_secs,
            encoding,
            representation: representation.to_string(),
            node: node.to_string(),
            state: RefCell::default(),
        })
    }

    /// Why it stopped hearing, where it did.
    pub fn stopped(&self) -> Option<CliError> {
        self.state.borrow_mut().stopped.take()
    }

    /// Waits for the device to play out what it holds.
    pub fn finish(self) -> Result<Played, CliError> {
        let Some(mut aplay) = self.state.into_inner().playing else {
            return Err(CliError::Io(format!(
                "the render pulled no block to play on `alsa:{}`",
                self.device
            )));
        };
        let frames = aplay.writer.as_ref().map_or(0, Writer::frames);
        let (played, said) = aplay.ended()?;
        if !played {
            return Err(refused(&self.device, &said));
        }
        inexact(&self.device, aplay.rate, &said)?;
        let rate = aplay.rate;
        Ok(Played {
            representation: self.representation,
            device: format!("alsa:{}", self.device),
            seconds: frames as f64 / f64::from(rate),
            rate,
            channels: aplay.channels,
            format: format(self.encoding),
            buffer_secs: self.buffer_secs,
            granted_buffer_secs: granted(&said).map(|frames| frames as f64 / f64::from(rate)),
            underruns: underruns(&said),
            underruns_seen_by: SEEN_BY,
        })
    }

    fn play(&self, state: &mut State, block: &Buffer) -> Result<(), CliError> {
        if state.playing.is_none() {
            state.playing = Some(self.spawn(block.width(), block.rate)?);
        }
        let aplay = state.playing.as_mut().expect("spawned above");
        let exited = aplay.child.try_wait();
        let exited = exited.map_err(|e| CliError::Io(format!("could not poll `aplay`: {e}")))?;
        if exited.is_some() {
            let (_, said) = aplay.ended()?;
            return Err(refused(&self.device, &said));
        }
        let writer = aplay.writer.as_mut().expect("open until it ends");
        match writer.block(block) {
            Ok(()) => {}
            Err(Unwritten::Sample(e)) => return Err(e),
            Err(Unwritten::Sink(e)) => {
                return Err(match aplay.ended()? {
                    (false, said) => refused(&self.device, &said),
                    (true, _) => CliError::Io(format!(
                        "`alsa:{}` stopped taking samples: {e}",
                        self.device
                    )),
                });
            }
        }
        let said = aplay.said.lock().expect("the reader never panics").clone();
        inexact(&self.device, aplay.rate, &said)
    }

    fn spawn(&self, channels: usize, rate: u32) -> Result<Aplay, CliError> {
        let buffer_us = (self.buffer_secs * 1e6).round() as i64;
        let spawned = Command::new("aplay")
            .args(["-v", "-D", &self.device, "-t", "raw", "-f"])
            .arg(format(self.encoding))
            .args(["-r", &rate.to_string(), "-c", &channels.to_string()])
            .arg(format!("--buffer-time={buffer_us}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(no_aplay(&self.device));
            }
            Err(e) => return Err(CliError::Io(format!("could not run `aplay`: {e}"))),
        };
        let mut stdin = child.stdin.take().expect("piped above");
        let (queue, queued) = mpsc::sync_channel::<Vec<u8>>(AHEAD);
        let feeder = std::thread::spawn(move || {
            for bytes in queued {
                if stdin.write_all(&bytes).is_err() {
                    return;
                }
            }
        });
        let stderr = child.stderr.take().expect("piped above");
        let said = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&said);
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stderr).split(b'\n') {
                let Ok(line) = line else {
                    return;
                };
                let line = String::from_utf8_lossy(&line).into_owned();
                heard.lock().expect("the writer never panics").push(line);
            }
        });
        Ok(Aplay {
            child,
            writer: Some(Writer::new(Queued(queue), self.encoding, &self.node)),
            feeder: Some(feeder),
            said,
            reader: Some(reader),
            rate,
            channels,
        })
    }
}

impl Hear for Hearing {
    fn heard(&self, block: &Buffer) -> bool {
        let mut state = self.state.borrow_mut();
        match self.play(&mut state, block) {
            Ok(()) => true,
            Err(e) => {
                state.stopped = Some(e);
                false
            }
        }
    }
}

/// Hands each block's bytes to the thread feeding aplay, so the render computes on while the
/// device plays what it holds, until it is `AHEAD` blocks ahead.
struct Queued(mpsc::SyncSender<Vec<u8>>);

impl Write for Queued {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self.0.send(bytes.to_vec()) {
            Ok(()) => Ok(bytes.len()),
            Err(_) => Err(std::io::ErrorKind::BrokenPipe.into()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Aplay {
    /// Closes its input once every queued byte is fed, waits for it to play out and exit, and
    /// hands back whether it exited clean and all it said.
    fn ended(&mut self) -> Result<(bool, Vec<String>), CliError> {
        drop(self.writer.take());
        if let Some(feeder) = self.feeder.take() {
            feeder.join().expect("the feeder never panics");
        }
        let status = self
            .child
            .wait()
            .map_err(|e| CliError::Io(format!("could not wait for `aplay`: {e}")))?;
        if let Some(reader) = self.reader.take() {
            reader.join().expect("the reader never panics");
        }
        let said = self.said.lock().expect("the reader never panics").clone();
        Ok((status.success(), said))
    }
}

impl Drop for Aplay {
    /// A render that stopped early leaves no player behind it.
    fn drop(&mut self) {
        if self.writer.take().is_some() {
            let _ = self.child.kill();
            if let Some(feeder) = self.feeder.take() {
                let _ = feeder.join();
            }
            let _ = self.child.wait();
        }
    }
}

/// The reason aplay gave, under the code a caller branches on.
fn refused(device: &str, said: &[String]) -> CliError {
    let reason: Vec<&str> = said
        .iter()
        .map(|line| line.trim())
        .filter(|line| {
            line.starts_with("aplay:")
                || line.starts_with("ALSA lib")
                || line.starts_with("Warning")
        })
        .collect();
    let reason = reason.join("; ");
    let (code, why) = if reason.contains("Device or resource busy") {
        ("destination.device_busy", "is busy")
    } else if reason.contains("audio open error") {
        ("destination.unknown_device", "could not be opened")
    } else {
        ("destination.device_failed", "failed")
    };
    CliError::usage(
        code,
        format!(
            "`alsa:{device}` {why}; aplay said: {reason}. `aplay -L` lists the devices there are"
        ),
    )
}

fn no_aplay(device: &str) -> CliError {
    CliError::usage(
        "destination.no_aplay",
        format!(
            "`alsa:{device}` plays through `aplay`, from alsa-utils, and no `aplay` is on PATH; \
             install alsa-utils"
        ),
    )
}

/// aplay plays on at the rate the device gave, whenever it says so.
fn inexact(device: &str, rate: u32, said: &[String]) -> Result<(), CliError> {
    match said.iter().find(|line| line.contains(INEXACT_RATE)) {
        Some(line) => Err(CliError::usage(
            "destination.rate_inexact",
            format!(
                "`alsa:{device}` cannot play {rate} Hz, and aplay said `{}`; render at a rate the \
                 device takes with `--rate`, or name `plughw:` or `default`, which resample",
                line.trim()
            ),
        )),
        None => Ok(()),
    }
}

fn underruns(said: &[String]) -> usize {
    said.iter().filter(|line| line.contains(UNDERRUN)).count()
}

/// The frames of the first `buffer_size` line `aplay -v` prints: the PCM it opened, ahead of
/// any slave beneath it.
fn granted(said: &[String]) -> Option<u64> {
    said.iter().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "buffer_size").then(|| value.trim().parse().ok())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    fn code(refused: &CliError) -> &'static str {
        match refused {
            CliError::Usage { code, .. } => code,
            other => panic!("refused as an argument mistake: {other:?}"),
        }
    }

    /// aplay's own lines, as it prints them for a device that is not there or is taken.
    #[test]
    fn a_device_aplay_could_not_open_refuses_under_its_own_code_with_aplay_s_reason() {
        let unknown = said(&[
            "ALSA lib pcm.c:2721:(snd_pcm_open_noupdate) Unknown PCM nosuch",
            "aplay: main:834: audio open error: No such file or directory",
        ]);
        let refused_unknown = refused("nosuch", &unknown);
        assert_eq!(code(&refused_unknown), "destination.unknown_device");
        assert!(refused_unknown.message().contains("Unknown PCM nosuch"));
        let busy = said(&["aplay: main:834: audio open error: Device or resource busy"]);
        let refused_busy = refused("hw:1", &busy);
        assert_eq!(code(&refused_busy), "destination.device_busy");
        assert!(refused_busy.message().contains("`alsa:hw:1` is busy"));
        let format = said(&[
            "Playing raw data 'stdin' : Float 32 bit Little Endian, Rate 48000 Hz, Stereo",
            "aplay: set_params:1343: Sample format non available",
        ]);
        let refused_format = refused("hw:1", &format);
        assert_eq!(code(&refused_format), "destination.device_failed");
        assert!(
            refused_format
                .message()
                .contains("Sample format non available")
        );
        assert!(!refused_format.message().contains("Playing raw data"));
    }

    #[test]
    fn a_rate_the_device_could_not_take_refuses_with_what_aplay_said() {
        let lines = said(&["Warning: rate is not accurate (requested = 44100Hz, got = 48000Hz)"]);
        let Err(refused) = inexact("hw:1", 44_100, &lines) else {
            panic!("refused");
        };
        assert_eq!(code(&refused), "destination.rate_inexact");
        assert!(refused.message().contains("got = 48000Hz"));
        assert!(inexact("hw:1", 48_000, &said(&["  rate : 48000"])).is_ok());
    }

    #[test]
    fn every_underrun_aplay_reports_is_counted_and_the_buffer_it_was_granted_read() {
        let lines = said(&[
            "Playing raw data 'stdin' : Float 32 bit Little Endian, Rate 48000 Hz, Stereo",
            "Plug PCM: Rate conversion PCM (44100, sformat=S16_LE)",
            "  buffer_size  : 2400",
            "  buffer_size  : 2205",
            "underrun!!! (at least 12.345 ms long)",
            "underrun!!! (at least 0.512 ms long)",
        ]);
        assert_eq!(underruns(&lines), 2);
        assert_eq!(granted(&lines), Some(2400));
        assert_eq!(underruns(&said(&["  buffer_size  : 2400"])), 0);
        assert_eq!(granted(&said(&["aplay: main:834: audio open error"])), None);
    }
}
