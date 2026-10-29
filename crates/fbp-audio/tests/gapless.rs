//! The property P0 exists to prove: a playlist is one continuous sample stream.
//!
//! Two WAVs at the device rate, played through `Sequence`: the output must be the exact
//! sample concatenation of the two files — not one sample of silence inserted, not one
//! dropped, and no re-open of anything at the boundary.

use std::path::{Path, PathBuf};

use fbp_audio::{Block, Decoder, PullState, Sequence, Shared};
use fbp_core::Track;

const RATE: u32 = 44_100;

/// Committed fixtures: two 2 s LAME mp3s, so the encoder-delay path is tested without
/// needing ffmpeg at test time.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fbp-p0-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Stereo 16-bit WAV: left a sine, right the same sine phase-shifted, so a channel
/// mix-up shows as a broken comparison.
fn write_wav(path: &Path, rate: u32, frames: usize, hz: f32) {
    let mut pcm = Vec::with_capacity(frames * 4);
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        let left = (t * hz * std::f32::consts::TAU).sin() * 0.5;
        let right = (t * hz * std::f32::consts::TAU + 0.5).sin() * 0.5;
        for value in [left, right] {
            pcm.extend_from_slice(&((value * 32_767.0) as i16).to_le_bytes());
        }
    }

    let data_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    std::fs::write(path, wav).expect("write wav");
}

fn decode_all(path: &Path) -> Vec<f32> {
    let mut decoder = Decoder::open(path).expect("open");
    let mut out = Vec::new();
    while let Some(block) = decoder.next_block() {
        out.extend_from_slice(block);
    }
    out
}

#[test]
fn the_playlist_is_the_exact_concatenation_of_its_files() {
    let dir = temp_dir("concat");
    let first = dir.join("a.wav");
    let second = dir.join("b.wav");
    write_wav(&first, RATE, 5_000, 440.0);
    write_wav(&second, RATE, 3_000, 660.0);

    let expected = {
        let mut expected = decode_all(&first);
        expected.extend(decode_all(&second));
        expected
    };

    let mut sequence = Sequence::new(
        vec![Track::new(first.clone()), Track::new(second.clone())],
        RATE,
    );
    let mut played = Vec::new();
    let mut tracks = Vec::new();
    while let Some(block) = sequence.next_block() {
        assert!(!block.samples.is_empty(), "an empty block would be a gap");
        tracks.push(block.track);
        played.extend_from_slice(&block.samples);
    }

    assert_eq!(
        played.len(),
        expected.len(),
        "sample count changed at a boundary"
    );
    assert_eq!(played, expected, "samples differ: gapless is broken");
    assert_eq!(played.len(), (5_000 + 3_000) * 2);

    let mut switches = tracks.windows(2).filter(|w| w[0] != w[1]);
    assert_eq!(switches.next(), Some(&[0u32, 1u32][..]));
    assert_eq!(switches.next(), None);
}

/// `enable_gapless` is what keeps an mp3 from opening with ~1105 samples of encoder
/// delay. Two seconds at 44.1 kHz must decode to two seconds.
#[test]
fn mp3_encoder_delay_is_trimmed() {
    for name in ["tone-a.mp3", "tone-b.mp3"] {
        let path = fixture(name);
        let frames = decode_all(&path).len() / 2;
        assert!(
            (frames as i64 - 88_200).abs() <= 256,
            "{name}: expected ~88200 frames, got {frames}"
        );
    }
}

/// The same two mp3s through the sequence: no silent block at the seam.
#[test]
fn mp3_tracks_join_without_silence() {
    let tracks = vec![
        Track::new(fixture("tone-a.mp3")),
        Track::new(fixture("tone-b.mp3")),
    ];
    let mut sequence = Sequence::new(tracks, RATE);
    let mut played = Vec::new();
    while let Some(block) = sequence.next_block() {
        played.extend_from_slice(&block.samples);
    }
    let frames = played.len() / 2;
    assert!(
        (frames as i64 - 2 * 88_200).abs() <= 512,
        "expected ~176400 frames for two 2 s files, got {frames}"
    );
    let seam = 88_200usize;
    let window = &played[(seam - 256) * 2..(seam + 256) * 2];
    let peak = window.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.05, "the seam is silent: peak {peak}");
}

#[test]
fn a_broken_file_is_an_error_row_and_the_stream_continues() {
    let dir = temp_dir("broken");
    let broken = dir.join("broken.mp3");
    let good = dir.join("good.wav");
    std::fs::write(&broken, b"not an mp3 at all").expect("write");
    write_wav(&good, RATE, 1_000, 440.0);

    let mut sequence = Sequence::new(vec![Track::new(broken), Track::new(good)], RATE);
    let mut played = 0usize;
    while let Some(block) = sequence.next_block() {
        played += block.samples.len();
    }
    let errors = sequence.take_errors();
    assert_eq!(
        errors.len(),
        1,
        "the broken file is reported once: {errors:?}"
    );
    assert_eq!(errors[0].0, 0);
    assert_eq!(played, 1_000 * 2, "the good file still played, in full");
}

#[test]
fn probe_reads_a_duration_without_decoding() {
    let dir = temp_dir("probe");
    let path = dir.join("duration.wav");
    write_wav(&path, RATE, RATE as usize, 440.0);
    let duration = fbp_audio::probe_duration_ms(&path).expect("probe");
    assert!((duration as i64 - 1_000).abs() <= 2, "got {duration} ms");
}

/// The callback path, driven without a sound card.
#[test]
fn fill_output_stitches_blocks_and_tracks_the_track() {
    use std::sync::mpsc::sync_channel;

    let (tx, rx) = sync_channel::<Block>(4);
    let shared = Shared::new(RATE);
    let mut state = PullState::new();

    tx.send(Block {
        track: 0,
        generation: 0,
        samples: vec![0.1, 0.2, 0.3, 0.4],
    })
    .unwrap();
    tx.send(Block {
        track: 0,
        generation: 0,
        samples: vec![0.5, 0.6],
    })
    .unwrap();

    let mut data = vec![0.0f32; 8];
    fbp_audio::fill_output(&mut data, &rx, &shared, &mut state);
    assert_eq!(data, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.0, 0.0]);
    assert_eq!(shared.track(), 0);
    assert_eq!(shared.position_ms(), 3 * 1000 / RATE as u64);

    // A new track starts: the track index and the position clock reset at the boundary.
    tx.send(Block {
        track: 1,
        generation: 0,
        samples: vec![0.7, 0.8],
    })
    .unwrap();
    fbp_audio::fill_output(&mut data, &rx, &shared, &mut state);
    assert_eq!(data[..2], [0.7, 0.8]);
    assert_eq!(shared.track(), 1);
    assert_eq!(shared.position_ms(), 1000 / RATE as u64);
}

#[test]
fn pause_is_silence_without_consuming_the_queue() {
    use std::sync::mpsc::sync_channel;

    let (tx, rx) = sync_channel::<Block>(4);
    let shared = Shared::new(RATE);
    let mut state = PullState::new();
    tx.send(Block {
        track: 0,
        generation: 0,
        samples: vec![0.25, 0.25],
    })
    .unwrap();

    shared.set_paused(true);
    let mut data = vec![1.0f32; 4];
    fbp_audio::fill_output(&mut data, &rx, &shared, &mut state);
    assert_eq!(data, vec![0.0; 4], "paused must output silence");
    assert_eq!(shared.position_ms(), 0, "paused must not advance the clock");

    shared.set_paused(false);
    fbp_audio::fill_output(&mut data, &rx, &shared, &mut state);
    assert_eq!(data[..2], [0.25, 0.25], "resume picks up where it stopped");
}

#[test]
fn blocks_from_a_skipped_track_are_dropped() {
    use std::sync::mpsc::sync_channel;

    let (tx, rx) = sync_channel::<Block>(4);
    let shared = Shared::new(RATE);
    let mut state = PullState::new();

    shared.bump_generation();
    tx.send(Block {
        track: 0,
        generation: 0,
        samples: vec![0.9, 0.9],
    })
    .unwrap();
    tx.send(Block {
        track: 1,
        generation: 1,
        samples: vec![0.1, 0.1],
    })
    .unwrap();

    let mut data = vec![0.0f32; 2];
    fbp_audio::fill_output(&mut data, &rx, &shared, &mut state);
    assert_eq!(
        data,
        [0.1, 0.1],
        "the stale block must be discarded, not played"
    );
    assert_eq!(shared.track(), 1);
}

#[test]
fn volume_scales_the_output() {
    use std::sync::mpsc::sync_channel;

    let (tx, rx) = sync_channel::<Block>(4);
    let shared = Shared::new(RATE);
    let mut state = PullState::new();
    shared.set_volume(0.5);
    tx.send(Block {
        track: 0,
        generation: 0,
        samples: vec![1.0, -0.5],
    })
    .unwrap();

    let mut data = vec![0.0f32; 2];
    fbp_audio::fill_output(&mut data, &rx, &shared, &mut state);
    assert_eq!(data, [0.5, -0.25]);
}
