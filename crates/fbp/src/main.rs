//! The fbp front-end.
//!
//! P0 shipped the audio path; P1a ships the library behind it. The front-end is still a
//! raw-mode key loop and a status line — the panel tree, the layout engine and the art
//! land in P1b — but every verb below goes through the same `fbp-index` API the panels
//! will, so the TUI is a renderer, not a second implementation.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use fbp_core::{is_audio, Track};
use fbp_index::{Library, ScanOptions, ScanProgress, Search, Sort};

const USAGE: &str = "\
fbp — a library player (P1a: the index)

USAGE:
    fbp [OPTIONS] [VERB]

VERBS (default: play the library):
    play [TEXT]             play everything, or everything matching TEXT
    scan                    index the library root (see --root)
    search <TEXT>           print matching tracks
    stats                   library counts and codecs

OPTIONS:
    -r, --root <DIR>        folder to walk (default: the last scanned root)
    -d, --db <FILE>         index file (default: <data dir>/fbp/library.db)
    -l, --list              print the walked files and exit
        --decode-check <F>  decode one file, print rate/frames/hash, exit
    -v, --volume <0..1>     starting volume (default: 1.0)
        --threads <N>       scanner reader threads (0 = auto)
        --no-tags           scan paths and metadata only, read no tags
    -h, --help              this

KEYS (playing):
    space  pause/resume      n  next      p  previous
    + / -  volume            q  quit
";

#[derive(Debug)]
enum Verb {
    Play(Option<String>),
    Scan,
    Search(String),
    Stats,
}

struct Args {
    root: Option<PathBuf>,
    db: Option<PathBuf>,
    list: bool,
    decode_check: Option<PathBuf>,
    volume: f32,
    threads: usize,
    read_tags: bool,
    verb: Verb,
}

fn main() -> ExitCode {
    // A piped-to-`head` CLI must exit quietly: Rust ignores SIGPIPE by default, so the
    // first write after the reader leaves panics with a backtrace instead of stopping.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fbp: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let args = parse(args)?;

    if let Some(path) = args.decode_check.as_deref() {
        return run_decode_check(path);
    }

    // `--list` and an explicit `--root` mean "the walk", exactly as in P0: no database.
    if args.list || (args.root.is_some() && matches!(args.verb, Verb::Play(None))) {
        let root = args.root.clone().unwrap_or_else(|| PathBuf::from("."));
        let tracks = walk(&root)?;
        if tracks.is_empty() {
            return Err(format!("no audio files under {}", root.display()));
        }
        if args.list {
            for track in &tracks {
                println!("{}", track.path.display());
            }
            return Ok(());
        }
        return play(tracks, args.volume);
    }

    let db = args.db.clone().unwrap_or_else(default_db_path);
    let mut library = Library::open(&db).map_err(|e| e.to_string())?;

    match args.verb {
        Verb::Scan => run_scan(
            &mut library,
            args.root.as_deref(),
            &db,
            ScanOptions {
                read_tags: args.read_tags,
                threads: args.threads,
            },
        ),
        Verb::Search(text) => run_search(&library, &text),
        Verb::Stats => run_stats(&library, &db),
        Verb::Play(query) => {
            let search = Search {
                text: query.unwrap_or_default(),
                sort: Sort::Artist,
                ..Default::default()
            };
            let paths = library.paths(&search).map_err(|e| e.to_string())?;
            if paths.is_empty() {
                return Err(format!(
                    "nothing to play in {} — run: fbp scan --root <DIR>",
                    db.display()
                ));
            }
            play(paths.into_iter().map(Track::new).collect(), args.volume)
        }
    }
}

fn parse(args: Vec<String>) -> Result<Args, String> {
    let mut parsed = Args {
        root: None,
        db: None,
        list: false,
        decode_check: None,
        volume: 1.0,
        threads: 0,
        read_tags: true,
        verb: Verb::Play(None),
    };

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-r" | "--root" => {
                index += 1;
                parsed.root = Some(PathBuf::from(next(&args, index, "--root")?));
            }
            "-d" | "--db" => {
                index += 1;
                parsed.db = Some(PathBuf::from(next(&args, index, "--db")?));
            }
            "-l" | "--list" => parsed.list = true,
            "--decode-check" => {
                index += 1;
                parsed.decode_check = Some(PathBuf::from(next(&args, index, "--decode-check")?));
            }
            "-v" | "--volume" => {
                index += 1;
                parsed.volume = next(&args, index, "--volume")?
                    .parse()
                    .map_err(|_| "--volume expects a number 0..1".to_string())?;
            }
            "play" => {
                // `play` takes the rest of the line as its query, so a title needs no quotes' worth of escaping.
                let rest = args[index + 1..].join(" ");
                parsed.verb = Verb::Play((!rest.trim().is_empty()).then_some(rest));
                index = args.len();
            }
            "scan" => parsed.verb = Verb::Scan,
            "search" => {
                let text = args[index + 1..].join(" ");
                if text.trim().is_empty() {
                    return Err("search needs some text".to_string());
                }
                parsed.verb = Verb::Search(text);
                index = args.len();
            }
            "--threads" => {
                index += 1;
                parsed.threads = next(&args, index, "--threads")?
                    .parse()
                    .map_err(|_| "--threads expects a number".to_string())?;
            }
            "--no-tags" => parsed.read_tags = false,
            "stats" => parsed.verb = Verb::Stats,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument `{other}` (try --help)")),
        }
        index += 1;
    }
    Ok(parsed)
}

fn next(args: &[String], index: usize, flag: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn default_db_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fbp")
        .join("library.db")
}

/// Every audio file under `root`, in path order. P0's walk, unchanged: it is what
/// `--list` means and what you get before there is an index.
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

/// Scan off the calling thread and report from the progress atomics, which is exactly what
/// the TUI will do in P1b (SCOPE: a scan never blocks input).
fn run_scan(
    library: &mut Library,
    root: Option<&Path>,
    db: &Path,
    options: ScanOptions,
) -> Result<(), String> {
    let root = match root {
        Some(root) => root.to_path_buf(),
        None => match library.root() {
            Some(root) => PathBuf::from(root),
            None => dirs::audio_dir().unwrap_or_else(|| PathBuf::from("~/Music")),
        },
    };

    println!("scanning {} into {}", root.display(), db.display());
    let progress = Arc::new(ScanProgress::new());

    // The scan runs here; a reporter thread polls the same atomics the TUI will. This is
    // the "a scan never blocks input" contract exercised with nothing but stdout.
    let reporter_progress = progress.clone();
    let reporter = std::thread::spawn(move || {
        use std::io::Write;
        while !reporter_progress.is_finished() {
            print!(
                "\r  {} / {} files, {} changed, {} errors   \x1b[K",
                reporter_progress.scanned(),
                reporter_progress.total(),
                reporter_progress.changed(),
                reporter_progress.errors()
            );
            let _ = std::io::stdout().flush();
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });

    let report = library
        .scan(&root, options, &progress)
        .map_err(|e| e.to_string())?;
    let _ = reporter.join();
    println!();
    println!(
        "{} files in {:.1}s ({} threads): {} added, {} updated, {} unchanged, {} missing, {} errors{}",
        report.scanned,
        report.elapsed.as_secs_f64(),
        report.threads,
        report.added,
        report.updated,
        report.unchanged,
        report.removed,
        report.errors,
        if report.cancelled { " (cancelled)" } else { "" }
    );
    for (path, message) in library.errors().map_err(|e| e.to_string())? {
        println!("  ! {path}: {message}");
    }
    Ok(())
}

fn run_search(library: &Library, text: &str) -> Result<(), String> {
    let search = Search {
        text: text.to_string(),
        sort: Sort::Artist,
        limit: Some(500),
        ..Default::default()
    };
    let rows = library.tracks(&search).map_err(|e| e.to_string())?;
    if rows.is_empty() {
        println!("no tracks match \"{text}\"");
        return Ok(());
    }
    println!(
        "{:>4}  {}  {}  trk  {}  {}  codec",
        "#",
        pad("artist", 22),
        pad("album", 22),
        pad("title", 34),
        pad("time", 6),
    );
    for (index, row) in rows.iter().enumerate() {
        println!(
            "{:>4}  {}  {}  {:>3}  {}  {}  {}",
            index + 1,
            pad(row.artist.as_deref().unwrap_or("—"), 22),
            pad(row.album.as_deref().unwrap_or("—"), 22),
            row.track_no
                .map(|n| n.to_string())
                .unwrap_or_else(|| "—".to_string()),
            pad(row.display_title(), 34),
            pad(&row.clock(), 6),
            row.codec.as_deref().unwrap_or("?"),
        );
    }
    println!("{} track(s)", rows.len());
    Ok(())
}

fn run_stats(library: &Library, db: &Path) -> Result<(), String> {
    let stats = library.stats().map_err(|e| e.to_string())?;
    println!("library  {}", db.display());
    println!(
        "root     {}",
        library.root().unwrap_or_else(|| "—".to_string())
    );
    println!(
        "tracks   {} ({} missing, {} unreadable)",
        stats.tracks, stats.missing, stats.errors
    );
    println!("albums   {}", stats.albums);
    println!("artists  {}", stats.artists);
    println!("playing  {}  {:.1} GiB", stats.clock(), stats.size_gib());
    for (codec, count) in &stats.by_codec {
        println!("  {codec:<8} {count}");
    }
    Ok(())
}

fn pad(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
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
