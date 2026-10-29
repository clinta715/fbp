//! The gapless core: a playlist as **one continuous sample stream**.
//!
//! The producer asks for the next block and gets audio for "the current track"; when a
//! file ends, `Sequence` opens the next one *in the same call* and keeps emitting. That
//! is the whole trick — no flush, no re-open of the device, no silence, and for
//! same-rate material not one added or dropped sample (pinned by `tests/gapless.rs`).
//!
//! A file that will not open becomes an error row and is skipped; the stream continues.

use std::path::PathBuf;

use fbp_core::Track;

use crate::decode::Decoder;
use crate::resample::Resampler;

/// Interleaved stereo f32 at the device rate, tagged with the track it came from and
/// the generation it was produced in (a skip makes older blocks stale — see `engine`).
#[derive(Clone, Debug)]
pub struct Block {
    pub track: u32,
    pub generation: u32,
    pub samples: Vec<f32>,
}

struct Current {
    index: usize,
    decoder: Decoder,
    resampler: Option<Resampler>,
}

pub struct Sequence {
    tracks: Vec<PathBuf>,
    cursor: usize,
    current: Option<Current>,
    target_rate: u32,
    errors: Vec<(usize, String)>,
}

impl Sequence {
    pub fn new(tracks: Vec<Track>, target_rate: u32) -> Self {
        Self {
            tracks: tracks.into_iter().map(|t| t.path).collect(),
            cursor: 0,
            current: None,
            target_rate,
            errors: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// The playlist index that is sounding now, if any.
    pub fn current(&self) -> Option<usize> {
        self.current.as_ref().map(|c| c.index)
    }

    /// Drop the current track and move to the next one.
    pub fn skip(&mut self) {
        if let Some(current) = self.current.take() {
            self.cursor = current.index + 1;
        }
    }

    /// Drop the current track and re-open the previous one from its start.
    pub fn back(&mut self) {
        let from = self.current.take().map(|c| c.index).unwrap_or(self.cursor);
        self.cursor = from.saturating_sub(1);
    }

    /// Failures since the last call, as `(playlist index, message)`.
    pub fn take_errors(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.errors)
    }

    /// The next block of audio, or `None` when the playlist is finished.
    ///
    /// A block is never empty and never short of a track boundary: the resampler's tail
    /// is emitted before the next file is opened.
    pub fn next_block(&mut self) -> Option<Block> {
        loop {
            if self.current.is_none() {
                if self.cursor >= self.tracks.len() {
                    return None;
                }
                let index = self.cursor;
                match Decoder::open(&self.tracks[index]) {
                    Ok(decoder) => {
                        let resampler = Resampler::new(decoder.sample_rate(), self.target_rate);
                        self.current = Some(Current { index, decoder, resampler });
                    }
                    Err(e) => {
                        self.errors.push((index, e.to_string()));
                        self.cursor = index + 1;
                        continue;
                    }
                }
            }

            let current = self.current.as_mut().expect("just opened");
            let index = current.index as u32;

            match current.decoder.next_block() {
                Some(block) => {
                    let samples = match current.resampler.as_mut() {
                        None => block.to_vec(),
                        Some(resampler) => resampler.push(block),
                    };
                    if samples.is_empty() {
                        continue;
                    }
                    return Some(Block { track: index, generation: 0, samples });
                }
                None => {
                    let tail = match current.resampler.as_mut() {
                        Some(resampler) => resampler.finish(),
                        None => Vec::new(),
                    };
                    self.cursor = current.index + 1;
                    self.current = None;
                    if !tail.is_empty() {
                        return Some(Block { track: index, generation: 0, samples: tail });
                    }
                }
            }
        }
    }
}
