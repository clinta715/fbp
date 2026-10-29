# fbp

A local-file audio library manager and player with a **bitmap-projected TUI**: one
`ratatui` widget tree, projected to terminal cells, to a wgpu window, and to PNG
screenshots — with cover art, waveforms and scopes carried *into* the cell grid as
half-block pixels. foobar2000's function, fooyin's layout, a terminal's form.

Scope and decisions: [docs/SCOPE.md](docs/SCOPE.md).

## Status: P1a — the index (2026-10-29)

```bash
fbp scan --root ~/Music          # index it (no UI blocking, resumable, cancellable)
fbp stats                        # tracks, albums, artists, hours, codecs
fbp search bjork                 # FTS5 over title/artist/album/albumartist/genre, diacritics folded
fbp play love                    # play everything matching, gaplessly
fbp play                         # the whole library, in album order
```

Measured on a real 100,640-file library (RAID1 of two spinning disks): **warm rescan 0.5 s**, FTS
**3–21 ms**, cold tag read ~23 ms/file, 78 MB database, and 102 unparseable files kept as visible error
rows instead of disappearing.

## P0 — the audio path (landed 2026-10-29)

```bash
cargo run -- --root ~/Music          # walk a folder and play it
cargo run -- --root ~/Music --list    # just the walk
cargo run -- --decode-check song.mp3  # headless: rate, frames, sample hash
cargo build --no-default-features     # no ALSA, no cpal, no audio out
```

Keys: `space` pause/resume, `n`/`p` next/previous, `+`/`-` volume, `q` quit.

**Proven in P0** (`cargo test`):

- **Gapless** — the playlist is one continuous sample stream: the output of a two-file
  sequence is *the exact sample concatenation* of the two files (no silence inserted,
  none dropped), and a file that will not open becomes an error row while the stream
  continues.
- **Pause/resume** is silence without consuming the queue, so resume is instant.
- **Skip** drops blocks already queued for the old track (a generation counter), so `n`
  is not heard late.
- **Resampling** to the device rate is per track and only when needed; same-rate albums
  pass through bit-exact.
- Walked the real 115k-file library (100,640 audio files) in **465 ms** (debug build).
- Verified on the machine: mp3 (44.1k) and flac decode, wav; **opus is listed and
  reported as unsupported** rather than hidden (SCOPE D3).

P0 is a raw-mode key loop and one status line on purpose; it dies when the panel tree
lands in P1.

## Crates

| Crate | Role |
|---|---|
| `fbp-core` | model types, no I/O |
| `fbp-audio` | decode (symphonia) → resample (rubato) → queue → cpal, plus the gapless sequence |
| `fbp` | the app: CLI, key loop (P0) → panels and layout (P1) |
