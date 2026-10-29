//! The transport, and the only place the `device` feature is mentioned.
//!
//! Panels ask this for the paused flag or the position; with `--no-default-features` it is
//! a no-op shell, so the whole UI (including `--shots`) builds and runs with no audio stack.

use fbp_core::Track;

#[cfg(feature = "device")]
use fbp_audio::Player;

#[derive(Default)]
pub struct Playback {
    #[cfg(feature = "device")]
    player: Option<Player>,
    #[allow(dead_code)]
    pub track: Option<String>,
    pub volume: f32,
}

impl Playback {
    pub fn new(volume: f32) -> Self {
        Self {
            volume,
            ..Default::default()
        }
    }

    /// Play `tracks` from the start, replacing whatever was playing. Returns why it could
    /// not, so the console panel can say so instead of nothing happening.
    pub fn play(&mut self, tracks: Vec<Track>) -> Option<String> {
        #[cfg(feature = "device")]
        {
            if self.player.is_some() {
                self.stop();
            }
            if tracks.is_empty() {
                return None;
            }
            match Player::open(tracks, self.volume) {
                Ok(player) => {
                    self.player = Some(player);
                    None
                }
                Err(error) => Some(error),
            }
        }
        #[cfg(not(feature = "device"))]
        {
            let _ = tracks;
            Some("built without the device feature: no audio output".to_string())
        }
    }

    pub fn toggle(&mut self) {
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            player.toggle_pause();
        }
    }

    pub fn next(&mut self) {
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            player.next();
        }
    }

    pub fn prev(&mut self) {
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            player.prev();
        }
    }

    pub fn nudge_volume(&mut self, delta: f32) -> f32 {
        self.volume = (self.volume + delta).clamp(0.0, 1.5);
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            player.set_volume(self.volume);
        }
        self.volume
    }

    pub fn paused(&self) -> bool {
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            return player.shared.paused();
        }
        true
    }

    pub fn position_ms(&self) -> u64 {
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            return player.position_ms();
        }
        0
    }

    /// Which track of the queue is sounding.
    pub fn index(&self) -> Option<usize> {
        #[cfg(feature = "device")]
        if let Some(player) = &self.player {
            return Some(player.track() as usize);
        }
        None
    }

    pub fn is_active(&self) -> bool {
        #[cfg(feature = "device")]
        if self.player.is_some() {
            return true;
        }
        false
    }

    pub fn stop(&mut self) {
        #[cfg(feature = "device")]
        if let Some(mut player) = self.player.take() {
            player.stop();
        }
    }
}
