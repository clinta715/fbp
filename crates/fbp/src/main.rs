//! P0 front-end: walk a folder, play it, show a status line.
//!
//! Deliberately *not* the TUI yet (that is P1). P0 has to prove exactly one thing — that
//! decode → resample → queue → device is sample-exact and gapless — so the front-end is a
//! raw-mode key loop and one status line. It dies when the panel tree lands.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fbp_core::{is_audio, Track};

const USAGE: &str = "\
fbp — a library player (P0: the audio path)

USAGE:
    fbp [OPTIONS]

OPTIONS:
    -r, --root <DIR>        folder to walk (default: .)
    -l, --list              print the walked files and exit
        --decode-check <F>  decode one file, print rate/frames/hash, exit
    -v, --volume <0..1>     starting volume (default: 1.0)
    -h, --help              this

KEYS (playing):
    space  pause/resume      n  next      p  previous
    + / -  volume            q  quit
";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fbp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut root: Option<PathBuf> = None;
    let mut list = false;
    let mut decode_check: Option<PathBuf> = None;
    let mut volume = 1.0f32;

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "-r" | "--root" => {
                index += 1;
                root = Some(PathBuf::from(next(&args, index, "--root")?));
            }
            "-l" | "--list" => list = true,
            "--decode-check" => {
                index += 1;
                decode_check = Some(PathBuf::from(next(&args, index, "--decode-check")?));
            }
            "-v" | "--volume" => {
                index += 1;
                volume = next(&args, index, "--volume")?
                    .parse()
                    .map_err(|_| "--volume expects a number 0..1".to_string())?;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown argument `{other}` (try --help)")),
        }
        index += 1;
    }

    if let Some(path) = decode_check {
        return run_decode_check(&path);
    }

    let root = root.unwrap_or_else(|| PathBuf::from("."));
    let tracks = walk(&root)?;
    if tracks.is_empty() {
        return Err(format!("no audio files under {}", root.display()));
    }
    if list {
        for track in &tracks {
            println!("{}", track.path.display());
        }
        return Ok(());
    }
    play(tracks, volume)
}

fn next(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Every audio file under `root`, in path order.
///
/// No index, no tags (P1): P0's walk is the smallest thing that can feed the decoder, and
/// it refuses to follow symlinked directories so a loop cannot hang it.
fn walk(root: &Path) -> Result<Vec<Track>, String> {
    if !root.exists() {
        return Err(format!("no such directory: {}", root.display()));
    }
    let mut tracks = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        if entry.file_type().is_file() && is_audio(entry.path()) {
            tracks.push(Track::new(entry.path()));
        }
    }
    tracks.sort();
    Ok(tracks)
}

/// The headless proof: decode a file and print what came out.
fn run_decode_check(path: &Path) -> Result<(), String> {
    let mut decoder = fbp_audio::Decoder::open(path).map_err(|e| e.to_string())?;
    let rate = decoder.sample_rate();
    let mut frames = 0usize;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    while let Some(block) = decoder.next_block() {
        frames += block.len() / 2;
        for sample in block {
            hash ^= sample.to_bits() as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    println!(
        "{}: {rate} Hz, {frames} frames, hash {hash:#018x}",
        path.display()
    );
    Ok(())
}

#[cfg(feature = "device")]
fn play(tracks: Vec<Track>, volume: f32) -> Result<(), String> {
    use std::collections::HashMap;
    use std::io::Write;
    use std::time::Duration;

    use fbp_audio::Player;
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use ratatui::crossterm::{cursor, execute, terminal};

    let total = tracks.len();
    let player = Player::open(tracks.clone(), volume)?;
    let mut durations: HashMap<u32, u64> = HashMap::new();
    let mut status_track = u32::MAX;
    let mut error_count = 0usize;
    let mut last_error = String::new();

    terminal::enable_raw_mode().map_err(|e| e.to_string())?;
    execute!(std::io::stdout(), cursor::Hide).map_err(|e| e.to_string())?;

    let outcome = (|| -> Result<(), String> {
        loop {
            if event::poll(Duration::from_millis(80)).map_err(|e| e.to_string())? {
                if let Event::Key(key) = event::read().map_err(|e| e.to_string())? {
                    // Plain keys only: a pty that hands us a NUL (Ctrl+Space) at startup
                    // must not toggle the transport.
                    let plain = !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    );
                    if key.kind == KeyEventKind::Press && plain {
                        match key.code {
                            KeyCode::Char(' ') => player.toggle_pause(),
                            KeyCode::Char('n') => player.next(),
                            KeyCode::Char('p') => player.prev(),
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                player.set_volume(player.shared.volume() + 0.05)
                            }
                            KeyCode::Char('-') => player.set_volume(player.shared.volume() - 0.05),
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            _ => {}
                        }
                    }
                }
            }

            for error in player.shared.take_errors() {
                error_count += 1;
                last_error = error;
            }
            if player.shared.finished() {
                break;
            }

            let track = player.track();
            if track != status_track {
                status_track = track;
                if let Some(entry) = tracks.get(track as usize) {
                    durations
                        .entry(track)
                        .or_insert_with(|| fbp_audio::probe_duration_ms(&entry.path).unwrap_or(0));
                }
            }

            let position = player.position_ms();
            let duration = durations.get(&track).copied().unwrap_or(0);
            let name = tracks
                .get(track as usize)
                .map(|t| t.file_name())
                .unwrap_or_else(|| "-".to_string());
            let state = if player.shared.paused() {
                "paused"
            } else {
                "playing"
            };
            let mut line = format!(
                "[{:>4}/{}] {}   {} / {}   {}   vol {:.0}%",
                track as usize + 1,
                total,
                name,
                clock(position),
                if duration == 0 {
                    "--:--".to_string()
                } else {
                    clock(duration)
                },
                state,
                player.shared.volume() * 100.0,
            );
            if error_count > 0 {
                line.push_str(&format!("   errors {error_count} (last: {last_error})"));
            }
            // A pty with no size set reports 0 columns; never truncate to nothing.
            let width = terminal::size()
                .map(|(w, _)| w as usize)
                .unwrap_or(120)
                .max(40);
            let line = truncate(&line, width.saturating_sub(1));
            print!("\r{line}\x1b[K");
            let _ = std::io::stdout().flush();
        }
        Ok(())
    })();

    let _ = execute!(std::io::stdout(), cursor::Show);
    let _ = terminal::disable_raw_mode();
    let mut player = player;
    player.stop();
    println!();
    outcome
}

#[cfg(not(feature = "device"))]
fn play(_tracks: Vec<Track>, _volume: f32) -> Result<(), String> {
    Err("built without the `device` feature: no audio output (try --list or --decode-check)".into())
}

#[cfg(feature = "device")]
fn clock(ms: u64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms / 1_000) % 60)
}

#[cfg(feature = "device")]
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}
