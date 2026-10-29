//! The audio path (SCOPE §7):
//!
//! ```text
//! decoder thread ─► resample ─► (gain) ─► queue ─► output callback
//!   (symphonia)     (rubato)             (blocks)   (cpal / test)
//! ```
//!
//! The property that makes gapless free: **the producer never stops between tracks.**
//! `Sequence` opens the next file as soon as the current one ends and keeps emitting
//! into the same stream, so for same-rate material the output is the sample-exact
//! concatenation of the files — no silence inserted, none dropped (pinned by
//! `tests/gapless.rs`).
//!
//! The producer/consumer split is the SCOPE's "one geometry" rule applied to control:
//! the front-end writes `Command`s, the callback reads `Shared` atomics, and
//! `fill_output` is the *only* place blocks become samples — so the device callback
//! and the tests run the same code.

mod decode;
mod engine;
mod resample;
mod sequence;

#[cfg(feature = "device")]
mod device;

pub use decode::{probe_duration_ms, Decoder};
pub use engine::{fill_output, Command, Player, PullState, Shared};
pub use resample::Resampler;
pub use sequence::{Block, Sequence};
