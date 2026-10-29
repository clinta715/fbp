//! The fbp front-end.
//!
//! No verb opens the TUI; `scan`, `search` and `stats` are the scriptable surface; `--shots`
//! renders the frames headlessly. All of them share `fbp-index`, so the CLI and the panels
//! cannot disagree about what the library contains.

mod config;
mod tui;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use fbp_core::{is_audio, Track};
use fbp_index::{Library, ScanOptions, ScanProgress, Search, Sort};

use config::{db_path, Config};

const USAGE: &str = "\
fbp — a TUI library player

USAGE:
    fbp [OPTIONS] [VERB]

VERBS (default: open the TUI):
    play [TEXT]             open the TUI and start playing (optionally filtered)
    scan                    index the library root (see --root)
    search <TEXT>           print matching tracks
    stats                   library counts and codecs

OPTIONS:
    -r, --root <DIR>        folder to walk (default: the config's root)
    -d, --db <FILE>         index file (default: <data dir>/fbp/library.db)
    -l, --list              print the walked files and exit
        --decode-check <F>  decode one file, print rate/frames/hash, exit
        --shots <DIR>       render the panels to PNGs and exit (needs --features shots)
    -v, --volume <0..1>     starting volume
        --threads <N>       scanner reader threads (0 = auto)
        --no-tags           scan paths and metadata only, read no tags
    -h, --help              this

KEYS:
    /  filter      Tab  focus     Enter  play / edit a tag field     space  pause/play
    n/p  next/prev  s  sort    S  scan    m M  tab stacks    u  undo    A  apply to filtered
    Ctrl+L  layout mode      Ctrl+S  save layout     Alt+1..9  load slot     q  quit
";

#[derive(Debug)]
enum Verb {
    Tui,
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
    shots: Option<PathBuf>,
    volume: Option<f32>,
    threads: usize,
    read_tags: bool,
    verb: Verb,
}

fn main() -> ExitCode {
    // A piped-to-head CLI must exit quietly: Rust ignores SIGPIPE, so the first write after
    // the reader leaves panics with a backtrace instead of stopping.
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
    let mut config = Config::load();
    if let Some(root) = &args.root {
        config.root = root.display().to_string();
    }
    if let Some(volume) = args.volume {
        config.volume = volume;
    }
    let db = args.db.clone().unwrap_or_else(db_path);

    if let Some(path) = args.decode_check.as_deref() {
        return run_decode_check(path);
    }
    if let Some(dir) = &args.shots {
        return run_shots(dir, &db, config);
    }
    if args.list {
        let tracks = walk(config.root_path())?;
        for track in &tracks {
            println!("{}", track.path.display());
        }
        return Ok(());
    }

    match args.verb {
        Verb::Tui => tui::run(db, config, None, false),
        Verb::Play(filter) => tui::run(db, config, filter, true),
        Verb::Scan => {
            let mut library = Library::open(&db).map_err(|e| e.to_string())?;
            run_scan(
                &mut library,
                config.root_path(),
                &db,
                ScanOptions {
                    read_tags: args.read_tags,
                    threads: args.threads,
                },
            )
        }
        Verb::Search(text) => {
            let library = Library::open(&db).map_err(|e| e.to_string())?;
            run_search(&library, &text)
        }
        Verb::Stats => {
            let library = Library::open(&db).map_err(|e| e.to_string())?;
            run_stats(&library, &db)
        }
    }
}

fn parse(args: Vec<String>) -> Result<Args, String> {
    let mut parsed = Args {
        root: None,
        db: None,
        list: false,
        decode_check: None,
        shots: None,
        volume: None,
        threads: 0,
        read_tags: true,
        verb: Verb::Tui,
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
            "--shots" => {
                index += 1;
                parsed.shots = Some(PathBuf::from(next(&args, index, "--shots")?));
            }
            "-v" | "--volume" => {
                index += 1;
                parsed.volume = Some(
                    next(&args, index, "--volume")?
                        .parse()
                        .map_err(|_| "--volume expects a number 0..1".to_string())?,
                );
            }
            "--threads" => {
                index += 1;
                parsed.threads = next(&args, index, "--threads")?
                    .parse()
                    .map_err(|_| "--threads expects a number".to_string())?;
            }
            "--no-tags" => parsed.read_tags = false,
            "play" => {
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

/// Every audio file under `root`, in path order. What `--list` means, before there is an
/// index.
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

/// Scan off the calling thread and report from the progress atomics -- the same contract the
/// TUI uses.
fn run_scan(
    library: &mut Library,
    root: &Path,
    db: &Path,
    options: ScanOptions,
) -> Result<(), String> {
    println!("scanning {} into {}", root.display(), db.display());
    let progress = Arc::new(ScanProgress::new());

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
        .scan(root, options, &progress)
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

#[cfg(feature = "shots")]
fn run_shots(dir: &Path, db: &Path, config: Config) -> Result<(), String> {
    let mut app = tui::app::App::open(db.to_path_buf(), config, None, false)?;
    let written = tui::shot::capture_all(&mut app, dir)?;
    for path in written {
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(not(feature = "shots"))]
fn run_shots(_dir: &Path, _db: &Path, _config: Config) -> Result<(), String> {
    Err("built without the `shots` feature: cargo build --release --features shots".into())
}
