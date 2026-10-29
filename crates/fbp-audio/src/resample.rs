//! Device-rate conversion, per track, only when the track's rate differs.
//!
//! Keyed on one property: a track that already matches the device rate is passed
//! through untouched (`Resampler::new` returns `None`), so the common case — an album
//! at one rate — stays sample-exact and free.

use rubato::{
    InterpolationParameters, InterpolationType, Resampler as _, SincFixedIn, WindowFunction,
};

const CHUNK: usize = 1024;
/// Taps in the interpolation filter. Its group delay is half of this, in *input*
/// frames — the frames a chunked resampler emits before the first real sample. P0
/// drops the matching output frames once per track so a converted track is not shifted
/// ~1.5 ms later than an unconverted one (the test pins the total length).
const SINC_LEN: usize = 128;

pub struct Resampler {
    inner: SincFixedIn<f32>,
    ratio: f64,
    skip: usize,
    fed: usize,
    emitted: usize,
    pending: Vec<f32>,
    left: Vec<f32>,
    right: Vec<f32>,
    out: Vec<f32>,
}

impl Resampler {
    /// `None` when no conversion is needed, or when the ratios are unusable.
    pub fn new(from: u32, to: u32) -> Option<Self> {
        if from == 0 || from == to {
            return None;
        }
        let ratio = to as f64 / from as f64;
        let params = InterpolationParameters {
            sinc_len: SINC_LEN,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: InterpolationType::Linear,
            window: WindowFunction::BlackmanHarris2,
        };
        let inner = SincFixedIn::<f32>::new(ratio, 1.1, params, CHUNK, 2).ok()?;
        let delay = ((SINC_LEN / 2) as f64 * ratio).round() as usize;
        Some(Self {
            inner,
            ratio,
            skip: delay,
            fed: 0,
            emitted: 0,
            pending: Vec::with_capacity(CHUNK * 4),
            left: Vec::with_capacity(CHUNK),
            right: Vec::with_capacity(CHUNK),
            out: Vec::with_capacity(CHUNK * 4),
        })
    }

    /// Feed interleaved stereo at the source rate; returns interleaved stereo at the
    /// target rate. Often empty: the resampler works a whole chunk at a time.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(interleaved);
        self.fed += interleaved.len() / 2;
        self.out.clear();
        while self.pending.len() / 2 >= CHUNK {
            self.chunk_from_pending(CHUNK);
            self.pending.drain(..CHUNK * 2);
            self.process();
        }
        self.emitted += self.out.len() / 2;
        std::mem::take(&mut self.out)
    }

    /// End of track: run the last partial chunk through the filter, then keep only the
    /// output that real input produced. Padding a whole chunk and emitting it would be
    /// a 23 ms hole at every track boundary.
    pub fn finish(&mut self) -> Vec<f32> {
        let have = self.pending.len() / 2;
        self.out.clear();
        if have == 0 {
            return Vec::new();
        }
        self.chunk_from_pending(have);
        self.pending.clear();
        self.process();
        // Owe exactly what the ratio says the input is worth, minus what was already
        // returned. The padded chunk above is comfortably longer than the filter's tail,
        // so this is the filter's own bookkeeping rather than a guess at its delay.
        let owed = ((self.fed as f64 * self.ratio).round() as usize).saturating_sub(self.emitted);
        if self.out.len() > owed * 2 {
            self.out.truncate(owed * 2);
        }
        self.emitted += self.out.len() / 2;
        std::mem::take(&mut self.out)
    }

    fn chunk_from_pending(&mut self, frames: usize) {
        self.left.clear();
        self.right.clear();
        for i in 0..frames {
            self.left.push(self.pending[2 * i]);
            self.right.push(self.pending[2 * i + 1]);
        }
        self.left.resize(CHUNK, 0.0);
        self.right.resize(CHUNK, 0.0);
    }

    fn process(&mut self) {
        let input = [&self.left[..], &self.right[..]];
        let Ok(planes) = self.inner.process(&input, None) else { return };
        let frames = planes[0].len().min(planes[1].len());
        for i in 0..frames {
            self.out.push(planes[0][i]);
            self.out.push(planes[1][i]);
        }
        if self.skip > 0 {
            let drop = self.skip.min(self.out.len() / 2);
            self.out.drain(..drop * 2);
            self.skip -= drop;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_needs_no_resampler() {
        assert!(Resampler::new(44_100, 44_100).is_none());
    }

    #[test]
    fn upsampling_preserves_length_and_amplitude() {
        let frames = 44_100;
        let input: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let s = (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5;
                [s, s]
            })
            .collect();
        let mut rs = Resampler::new(44_100, 48_000).expect("resampler");
        let mut out = rs.push(&input);
        out.extend(rs.finish());
        let out_frames = out.len() / 2;
        let want = 48_000;
        assert!(
            (out_frames as i64 - want as i64).abs() <= 16,
            "expected ~{want} frames, got {out_frames}"
        );
        // The tail is the filter's own ramp; the body must keep the signal.
        let peak = out[4_000 * 2..40_000 * 2]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.05, "peak drifted: {peak}");
    }
}
