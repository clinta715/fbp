//! The producer thread, the shared state the front-end reads, and `fill_output` — the
//! single function that turns queued blocks into samples.
//!
//! `fill_output` is called by the cpal callback in the app and directly by the tests,
//! so pause, volume, track-boundary bookkeeping and stale-block dropping are all
//! exercised without a sound card.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fbp_core::Track;

use crate::sequence::{Block, Sequence};

/// Blocks buffered ahead of the callback: 4 × ~23 ms ≈ 90 ms of headroom. Small enough
/// that a skip is not heard late, large enough to survive a scheduler hiccup.
const QUEUE_BLOCKS: usize = 4;

/// One control entry point for TUI, CLI and (later) MCP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Next,
    Prev,
    Pause,
    Resume,
    Volume(f32),
    Stop,
}

/// The read-only half of the player: atomics only, so a 20 fps status line and the
/// device callback never take a lock.
pub struct Shared {
    paused: AtomicBool,
    volume: AtomicU32,
    track: AtomicU32,
    generation: AtomicU32,
    frames: AtomicU64,
    track_start: AtomicU64,
    rate: u32,
    finished: AtomicBool,
    errors: Mutex<Vec<String>>,
}

impl Shared {
    pub fn new(rate: u32) -> Self {
        Self {
            paused: AtomicBool::new(false),
            volume: AtomicU32::new(1.0f32.to_bits()),
            track: AtomicU32::new(0),
            generation: AtomicU32::new(0),
            frames: AtomicU64::new(0),
            track_start: AtomicU64::new(0),
            rate: rate.max(1),
            finished: AtomicBool::new(false),
            errors: Mutex::new(Vec::new()),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume
            .store(volume.clamp(0.0, 1.5).to_bits(), Ordering::Relaxed);
    }

    pub fn track(&self) -> u32 {
        self.track.load(Ordering::Relaxed)
    }

    /// Frames of the current track already handed to the device. Leads the audible
    /// position by at most the queue depth (~90 ms) — P0 shows a clock, not a sync point.
    pub fn position_ms(&self) -> u64 {
        let frames = self.frames.load(Ordering::Relaxed);
        let start = self.track_start.load(Ordering::Relaxed);
        frames.saturating_sub(start).saturating_mul(1000) / self.rate as u64
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Relaxed)
    }

    pub fn bump_generation(&self) -> u32 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn push_error(&self, error: String) {
        if let Ok(mut errors) = self.errors.lock() {
            errors.push(error);
        }
    }

    /// Errors seen since the last call, already formatted for a status line.
    pub fn take_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }
}

/// What the callback carries between invocations: the block being drained and how far.
#[derive(Default)]
pub struct PullState {
    block: Option<Block>,
    offset: usize,
    track: u32,
}

impl PullState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Fill `data` (interleaved stereo, device rate) from the queue.
///
/// Paused means *silence without consuming*: the queue keeps its blocks, so resume is
/// instant instead of a refill. Blocks from a skipped track are dropped on arrival —
/// that is what makes `n` respond in milliseconds instead of queue-depth milliseconds.
pub fn fill_output(data: &mut [f32], rx: &Receiver<Block>, shared: &Shared, state: &mut PullState) {
    data.fill(0.0);
    if shared.paused() {
        return;
    }
    let volume = shared.volume();
    let generation = shared.generation();
    let base = shared.frames.load(Ordering::Relaxed);
    let mut written = 0usize;

    while written + 2 <= data.len() {
        if state.block.is_none() {
            match rx.try_recv() {
                Ok(block) => {
                    if block.generation != generation {
                        continue;
                    }
                    if block.track != state.track {
                        state.track = block.track;
                        shared.track.store(block.track, Ordering::Relaxed);
                        shared
                            .track_start
                            .store(base + (written / 2) as u64, Ordering::Relaxed);
                    }
                    state.offset = 0;
                    state.block = Some(block);
                }
                Err(_) => break,
            }
        }

        let Some(block) = state.block.as_ref() else {
            break;
        };
        let available = block.samples.len() - state.offset;
        let room = data.len() - written;
        let n = available.min(room) & !1;
        if n == 0 {
            break;
        }
        for i in 0..n {
            data[written + i] = block.samples[state.offset + i] * volume;
        }
        written += n;
        state.offset += n;
        if state.offset >= block.samples.len() {
            state.block = None;
        }
    }

    shared
        .frames
        .store(base + (written / 2) as u64, Ordering::Relaxed);
}

pub struct Player {
    pub shared: Arc<Shared>,
    ctrl: SyncSender<Command>,
    producer: Option<JoinHandle<()>>,
    #[cfg(feature = "device")]
    pub(crate) stream: Option<cpal::Stream>,
}

impl Player {
    /// Start the producer and hand back the consumer end. The caller decides what pulls
    /// from it: the cpal callback, or a test.
    pub fn spawn(tracks: Vec<Track>, rate: u32) -> (Self, Receiver<Block>) {
        let shared = Arc::new(Shared::new(rate));
        let (block_tx, block_rx) = sync_channel::<Block>(QUEUE_BLOCKS);
        let (ctrl_tx, ctrl_rx) = sync_channel::<Command>(16);

        let mut sequence = Sequence::new(tracks, rate);
        let producer_shared = shared.clone();
        let producer = thread::spawn(move || {
            let mut staged: Option<Block> = None;
            'run: loop {
                while let Ok(command) = ctrl_rx.try_recv() {
                    match command {
                        Command::Next => {
                            staged = None;
                            sequence.skip();
                        }
                        Command::Prev => {
                            staged = None;
                            sequence.back();
                        }
                        Command::Pause => producer_shared.set_paused(true),
                        Command::Resume => producer_shared.set_paused(false),
                        Command::Volume(v) => producer_shared.set_volume(v),
                        Command::Stop => break 'run,
                    }
                }

                for (index, error) in sequence.take_errors() {
                    producer_shared.push_error(format!("#{}: {error}", index + 1));
                }

                if producer_shared.paused() {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }

                if staged.is_none() {
                    match sequence.next_block() {
                        Some(block) => staged = Some(block),
                        None => break 'run,
                    }
                }

                let mut block = staged.take().expect("staged above");
                block.generation = producer_shared.generation();
                match block_tx.try_send(block) {
                    Ok(()) => {}
                    Err(TrySendError::Full(block)) => {
                        staged = Some(block);
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(TrySendError::Disconnected(_)) => break 'run,
                }
            }
            producer_shared.finished.store(true, Ordering::Relaxed);
        });

        let player = Self {
            shared,
            ctrl: ctrl_tx,
            producer: Some(producer),
            #[cfg(feature = "device")]
            stream: None,
        };
        (player, block_rx)
    }

    pub fn send(&self, command: Command) {
        let _ = self.ctrl.try_send(command);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    pub fn toggle_pause(&self) {
        if self.shared.paused() {
            self.resume()
        } else {
            self.pause()
        }
    }

    /// Skipping bumps the generation, so blocks already queued for the old track are
    /// discarded by the callback instead of being played first.
    pub fn next(&self) {
        self.shared.bump_generation();
        self.send(Command::Next);
    }

    pub fn prev(&self) {
        self.shared.bump_generation();
        self.send(Command::Prev);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.set_volume(volume);
        self.send(Command::Volume(volume));
    }

    pub fn position_ms(&self) -> u64 {
        self.shared.position_ms()
    }

    pub fn track(&self) -> u32 {
        self.shared.track()
    }

    pub fn stop(&mut self) {
        self.send(Command::Stop);
        #[cfg(feature = "device")]
        {
            self.stream = None;
        }
        if let Some(handle) = self.producer.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}
