//! One file → interleaved f32 stereo, at the file's own rate.
//!
//! Channel counts above stereo keep the first two channels for now (a real downmix
//! matrix is P2); mono is duplicated. Gapless trim is on, so an mp3's encoder
//! delay/padding is not played back as silence.

use std::fmt;
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as SymphoniaDecoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Why a file could not be played. Every variant is a *row* in the app, never a panic:
/// a library of 115k files always contains a few broken ones.
#[derive(Debug)]
pub enum Error {
    Open(String),
    Probe(String),
    NoTrack(String),
    Codec(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Open(s) => write!(f, "cannot open: {s}"),
            Error::Probe(s) => write!(f, "unsupported or corrupt container: {s}"),
            Error::NoTrack(s) => write!(f, "no audio track: {s}"),
            Error::Codec(s) => write!(f, "decoder: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<SymphoniaError> for Error {
    fn from(e: SymphoniaError) -> Self {
        Error::Codec(e.to_string())
    }
}

pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymphoniaDecoder>,
    track_id: u32,
    sample_rate: u32,
    stereo: Vec<f32>,
    done: bool,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let file = File::open(path).map_err(|e| Error::Open(format!("{}: {e}", path.display())))?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| Error::Probe(format!("{}: {e}", path.display())))?;

        let format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| Error::NoTrack(path.display().to_string()))?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let sample_rate = params.sample_rate.unwrap_or(44_100);
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;

        Ok(Self {
            format,
            decoder,
            track_id,
            sample_rate,
            stereo: Vec::new(),
            done: false,
        })
    }

    /// The rate of the samples this decoder returns (not the device's).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The next block of interleaved stereo f32, or `None` at the end of the file.
    ///
    /// A bad frame is skipped rather than ending the track: a scratched mp3 should
    /// lose a frame, not the record. A truncated file ends normally, with whatever
    /// decoded cleanly before the damage.
    pub fn next_block(&mut self) -> Option<&[f32]> {
        while !self.done {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymphoniaError::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => {
                    self.done = true;
                    return None;
                }
                Err(_) => {
                    self.done = true;
                    return None;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }

            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => {
                    self.done = true;
                    return None;
                }
            };

            let spec = *decoded.spec();
            let channels = spec.channels.count().max(1);
            let frames = decoded.frames();
            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            let samples = buf.samples();

            self.stereo.clear();
            self.stereo.reserve(frames * 2);
            if channels == 1 {
                for &s in samples {
                    self.stereo.push(s);
                    self.stereo.push(s);
                }
            } else {
                for frame in samples.chunks_exact(channels) {
                    self.stereo.push(frame[0]);
                    self.stereo.push(frame[1]);
                }
            }
            return Some(&self.stereo);
        }
        None
    }
}

/// Duration from the container/tag, without decoding a single frame.
///
/// This is what keeps a 115k-file scan at seconds instead of tens of minutes; `None`
/// means "the container did not say", and the front-end shows `--:--`.
pub fn probe_duration_ms(path: &Path) -> Option<u64> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .ok()?;
    let params = probed.format.default_track()?.codec_params.clone();
    let rate = params.sample_rate?;
    let frames = params.n_frames?;
    Some(frames.saturating_mul(1000) / rate as u64)
}
