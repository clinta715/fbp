//! cpal output — the only place a stream is built.
//!
//! Everything interesting lives in `engine::fill_output`; this module is the wiring
//! (default device, sample-format dispatch, stereo→N-channel fan-out) plus the rule that
//! the *device's* rate is the rate the whole pipeline runs at.

use std::sync::mpsc::Receiver;
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use fbp_core::Track;

use crate::engine::{fill_output, Player, PullState, Shared};
use crate::sequence::Block;

impl Player {
    /// Play `tracks` on the default output device, at the device's own rate.
    pub fn open(tracks: Vec<Track>, volume: f32) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        let (mut player, rx) = Player::spawn(tracks, config.sample_rate);
        player.shared.set_volume(volume);

        // One receiver, one branch: the arms are exclusive, so the option only exists to
        // satisfy the borrow checker.
        let mut receiver = Some(rx);
        let shared = player.shared.clone();
        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                build::<f32>(&device, &config, receiver.take().unwrap(), shared)?
            }
            cpal::SampleFormat::I16 => {
                build::<i16>(&device, &config, receiver.take().unwrap(), shared)?
            }
            cpal::SampleFormat::U16 => {
                build::<u16>(&device, &config, receiver.take().unwrap(), shared)?
            }
            other => return Err(format!("unsupported device sample format {other:?}")),
        };
        stream.play().map_err(|e| e.to_string())?;
        player.stream = Some(stream);
        Ok(player)
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    receiver: Receiver<Block>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let channels = (config.channels as usize).max(1);
    let mut state = PullState::new();
    let mut stereo: Vec<f32> = Vec::new();
    let error_shared = shared.clone();

    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _info| {
                let frames = data.len() / channels;
                if stereo.len() != frames * 2 {
                    // Grows once on the first callback, then never again.
                    stereo.resize(frames * 2, 0.0);
                }
                fill_output(&mut stereo, &receiver, &shared, &mut state);
                for frame in 0..frames {
                    for channel in 0..channels {
                        let sample = match channel {
                            0 => stereo[frame * 2],
                            1 => stereo[frame * 2 + 1],
                            _ => 0.0,
                        };
                        data[frame * channels + channel] = T::from_sample(sample);
                    }
                }
            },
            move |error| error_shared.push_error(format!("output stream: {error}")),
            None,
        )
        .map_err(|e| e.to_string())
}
