# fbp — scope

**Status:** scope doc, draft 4 (P0, P1a and P1b landed — see §10).
**Working name:** `fbp` (foobar, projected — rename freely).
**Reference implementation for the UI spine:** `/mnt/nvme2/build/htrk3` (DESIGN §UI strategy).
**Target library:** `~/Music` → `/mnt/md0/mp3`: **115,577 files**, 99,329 mp3, 671 opus, 1,654 m3u,
363 cue, 5,471 cover images. Every budget below is set against that number.

**Landed:** P0 the audio path · P1a the index (schema, incremental scanner, tags, FTS) · P1b the
layout engine, five panels, half-block cover art and headless PNG shots. P1c is MCP, playlist
files/cue sheets, and panels bound to saved views.

**Decided (draft 2):**
- **Linux only.** cpal stays portable as a side effect; macOS/Windows are untested, not targets.
- **No opus in v1.** 671 files are indexed and shown as *unsupported codec, needs libopus* — we are not
  writing a decoder. Revisit as a feature flag if an acceptable binding appears.
- **`--window` becomes the default** once the pixel panels (spectrum/scope/waveform) land; terminal
  stays first-class and is the SSH/tmux mode. Built default-off, designed as a peer from day one.
- **Library management is the weight centre.** Effects, DSP, transcoding and layout chrome come after
  tags, files, art, queries, playlists and repair — foobar's actual differentiator.

---

## 1. What it is

A **local-file audio library manager and player**: scan folders into an index, browse/sort/filter/
search it, build playlists, correct its tags, repair its file paths, play it gaplessly. foobar2000's
*function*, fooyin's *layout*, a terminal's *form* — plus a pixel projection for what a cell grid
cannot carry (cover art, waveform, spectrum).

Out of scope, deliberately: streaming/radio, transcoding/conversion, CD ripping, scrobbling, plugin
ABI, bit-perfect/exclusive output, mod/xm playback (773 files present — that is another app's job).

Why it is not `mpd`+`ncmpcpp` and not `termusic`: a real index (incremental scan, tags, albums, art,
ReplayGain, repair); a projection matrix that runs bitmaps both into and out of the cell grid; and one
handler layer the TUI, the CLI and an MCP server all call, so they cannot drift (§8).

---

## 2. The projection matrix (architectural spine)

Copied wholesale from htrk3 — it is already paid for and already debugged:

| Content | → cell grid (`--tui`) | → pixels (`--window`) |
|---|---|---|
| text, tables, trees, chrome | native `ratatui` widgets | same tree via `ratatui-wgpu` + winit |
| **cover art** | half-block cells (`▀`, fg/bg, truecolor) | RGBA texture quad |
| **waveform** | per-cell min/max bands (`▁▂▃▄…`) | `PostProcessor` overlay, 1 px per peak |
| **spectrum / scope / meters** | block columns, 2×1 px per cell | overlay bands, snapped |
| **seek cursor, markers** | a styled cell column | snapped 1 px quad column |

**Six invariants** (each is a bug htrk3 paid for — do not relearn them):

1. **The `ratatui` tree is the truth.** No retained scene graph. Exactly one narrow struct —
   `Overlay { rect, bands, shapes, rasters, rules }` — crosses the cell/pixel boundary. A panel
   publishes an `Overlay`; it never touches wgpu, kitty, a decoder or a resampler.
2. **One geometry, three consumers.** Band→cell, time→x, hit rect: plain functions used by the cell
   draw, the pixel overlay *and* the mouse hit-test, so a cursor, a click and an overlay rule cannot
   disagree.
3. **One app core.** `TuiApp::handle_term_event` is the single input path, `tick` the single per-frame
   body (transport, scan progress, art decode, meters), `chrome::render` the single frame. The windowed
   front-end is a backend, never a fork. Behaviour added in `window/` is a bug.
4. **Cadence is a deadline.** 20 fps idle, immediate redraw on input. A redraw requested in
   `about_to_wait` is an infinite spin (100 % of a core) — htrk3's first wgpu bug.
5. **Pixel snapping.** 1 px lines and 1 px quads on a pixel boundary cover nothing: everything is a
   **quad**, markers snap to a whole pixel column, traces use the theme's `text` token.
6. **`shots`** (feature): render any view into `TestBackend`, rasterise cells with `font8x8`, write
   PNG. Golden-free structural checks — and because art is drawn as block glyphs, **real cover art in
   the PNGs with no image code in the shot path**.

Consequence that makes the whole design cohere: *one art pipeline feeds the terminal, the window and
CI.* And a terminal scope is not a toy — at 200×60 cells a half-block spectrum is 200×120 truecolor
pixels. The window is for smoothness and for waveform detail, not for the concept.

---

## 3. Layout system (foobar's real feature, constrained)

foobar's soul is not one window, it is *your* window. We do a deliberately small version: a **tree of
splitters and tab stacks over a fixed panel vocabulary**, saved as editable TOML, with named
workspaces. Not foobar's full flexibility — no scripting, no UI DSL, no plugin panels, no floating
windows — but enough that two people's screens look different.

```toml
# ~/.config/fbp/layouts/default.toml  (as built; serde-tagged, so the file is one [node] table)
[node]
kind = "split"
axis = "horizontal"
weights = [26, 44, 30]          # cells, cell-granular

[[node.children]]
kind = "panel"
panel = "tree"

[[node.children]]
kind = "tabs"
active = 0

[[node.children.children]]
kind = "panel"
panel = "table"

[[node.children.children]]
kind = "panel"
panel = "albums"

[[node.children]]
kind = "split"
axis = "vertical"
weights = [62, 38]

[[node.children.children]]
kind = "panel"
panel = "nowplaying"

[[node.children.children]]
kind = "panel"
panel = "console"
```

- **Three node kinds.** `Panel`, `Tabs` (foobar's tab stack), `Split` (axis + weights). Depth ≤ 4,
  min panel 6×3 cells. That is enough for every layout in foobar's screenshot gallery that anyone
  actually uses.
- **Panels are instantiated, not singletons.** Two track tables side by side, each bound to a different
  **view** (a saved filter + column set + sort + group-by) is the single most-used foobar layout, so
  views are first-class rows in the DB, not per-window state.
- **Editing in-app.** `Ctrl+L` enters layout mode: `h`/`v` split the focused panel, `x` closes it,
  `t` cycles its panel kind, arrows resize (cell-granular), `Esc` leaves. `Ctrl+S` writes the current
  tree to `layouts/<config.layout>.toml`, so a good arrangement is one keystroke from being kept and
  a bad one one keystroke from being replaced.
- **Workspaces** `Alt+1..9` load `layouts/slotN.toml`; there is no in-app "save as slot" yet (P1c).
- **Layouts are shareable files** and every layout in `layouts/` is a `shots` target, so a layout
  change cannot silently break a panel's geometry.
- **Limit as built:** one instance per panel kind (a second `table` panel would share the first
  one's selection). Panels as *instances bound to saved views* is P1c, which is also when
  `views(id, name, filter, columns, sort, group_by)` gets a UI.

**Why this and the projection matrix are one mechanism:** a panel receives a `Rect` and publishes
cells plus an `Overlay`. It does not know whether that rect will be rasterised to glyphs or pixels, and
it degrades by *size class* alone (art below 8×4 cells → the album initial; a waveform below 10 cells
wide → a progress bar). So a layout authored in the terminal is the same layout in the window, and
vice versa. The layout system is what makes the seam pay off instead of being a special case.

---

## 4. Workspace

| Crate | Role |
|---|---|
| `fbp-core` | model types only (`Track`, `Album`, `Query`, `View`, `Layout`, `Overlay`, `Theme`): no I/O |
| `fbp-index` | SQLite schema + migrations, scanner, tags, art extract/dedupe/cache, FTS, query compiler |
| `fbp-files` | file operations: rename/move/organize by pattern, relocate, duplicate detection, atomic tag write |
| `fbp-audio` | decode (symphonia), resample (rubato), gain/fade, cpal out, command ring, peaks, FFT |
| `fbp-tools` | **the handler layer** — every mutation and query the app can perform (`LibraryHost` trait) |
| `fbp` | the app: `tui/` (app, panels, chrome, layout, theme, hit), `tui/window/` + `scene.rs`, `tui/shot.rs`, `cli.rs`, `mcp_server.rs`, `config.rs` |
| `fbp-fixtures` | generated test library (valid mp3/flac/wav, 6 albums, pathological names) + conformance tests |

Dependencies and why each is in the tree:

| Crate | Why |
|---|---|
| `ratatui` 0.30, `ratatui-wgpu` 0.6, `winit` 0.30, `pollster` | the two front-ends (feature `window`) |
| `symphonia` (mp3, flac, vorbis, aac/alac, wav) | in-process decode; the only path that can do gapless |
| `cpal` (feature `device`) | output stream |
| `rubato` | device-rate resampling |
| `lofty` | tags read **and write**, duration, embedded art, ID3v2.2–2.4 / Vorbis / APE / MP4 |
| `rusqlite` (bundled, FTS5) | the index: 100k rows is a database problem |
| `walkdir`, `nucleo`, `rustfft` | scan, quick filter, spectrum |
| `ringbuf` | UI→audio ring. **P0 shipped a bounded `sync_channel` instead** (no dep); swap when the realtime path is measured, not before |
| `image`, `png` | art decode/resize (thumb cache); PNG for `shots` |
| `font8x8` (optional, `shots`) | cell → PNG rasteriser |
| `serde`, `serde_json`, `toml`, `dirs` | config, layouts, MCP wire, session |
| `mpris-server` (P3) | media keys, `playerctl`, desktop integration |

**Copy, do not depend on, htrk3:** `tui/shot.rs`, `theme.rs`, the `Cadence`, the hit-test, the
command-ring audio skeleton, `window/mod.rs` and `window/scene.rs` — ~1.2k lines already debugged
there. A shared crate is a later decision; a cross-repo dependency today just couples two releases.

---

## 5. Index, scan, and the library-management inventory

SQLite, WAL, `$XDG_DATA_HOME/fbp/library.db`.

```
tracks(id, path UNIQUE, dir, filename, ext, size, mtime, inode, duration_ms, codec, sample_rate,
       channels, bitrate, title, artist, albumartist, album, track_no, disc_no, disc_total, year,
       genre, composer, comment, rg_track_gain, rg_track_peak, rg_album_gain, rg_album_peak,
       art_id, has_cue, added_at, last_played, play_count, skip_count, rating, missing, seen)
albums   = view over tracks (albumartist, album)  -- album art, album RG, year, disc count
art(id, hash UNIQUE, w, h, kind, source, thumb48, thumb96, thumb192)   -- deduped, pre-rendered
views(id, name, filter, columns, sort, group_by)   -- a saved query, bindable to any table panel
playlists(id, name, created, kind) / playlist_items(playlist_id, pos, track_id, path)
fts_tracks(title, artist, album, albumartist, genre, comment)   -- external-content FTS5
peaks(track_id, cols BLOB)     -- waveform, lazy, ~64 B/track
meta(key, value)               -- schema version, roots, last scan
```

**Measured on the real library** (`/mnt/md0/mp3`, a RAID1 of two spinning ST2000DM001s, 31 GB RAM):

| | |
|---|---|
| indexed | **100,640 files** → 18,181 albums, 11,931 artists, 447 d 20 h of audio, 1,061 GiB |
| database | 78 MB (WAL) |
| **warm rescan** (nothing changed) | **0.5–0.7 s** — the budget was < 5 s |
| cold tag read | **~23 ms/file** (a seek per file on this array) → a first-ever full scan ≈ 40 min serial |
| 8 readers, 26,912 cold files | 265 s (~**2.3×** serial; the disks are the limit, not the CPU) |
| FTS query | **3–21 ms** including process start and DB open |
| unparseable files | 102 → rows in `errors`, shown as "unreadable", never silently absent |

**Scan, as built (P1a).** One walk (directory entries only, no `stat`), one bulk `SELECT` of the
root's rows into memory, a metadata pass, then tag reads only for files whose
`(size, mtime, inode)` changed — not hashing: a full metadata pass over 99k files is 1.9 s. Eight
parallel readers, one writer batching 512 rows per transaction, removals computed in memory from the
paths seen, and a `(scanned, total, current_path, errors)` progress contract that is cancellable and
never blocks input. Duration comes from the container/tag, not from decoding. Cover art is **not**
parsed (`read_cover_art(false)`): art is a per-album lazy fetch, so indexing never pulls ~20 GB of
JPEGs through a spinning disk to fill a table. `errors` is keyed by path, so a warm rescan keeps
reporting a broken file until it is read again and works.

**Art.** Content-hashed and deduped (5,471 images for far fewer distinct covers — `folder.jpg` repeats
per rip). Priority: embedded > `cover/front/folder.jpg` in the album dir > heuristic filename. Thumbs
pre-rendered to 48/96/192 px so the draw path never resizes.

**Management inventory** — this is the real product, ordered by phase:

| Capability | Notes | Phase |
|---|---|---|
| incremental scan, ignore patterns, symlink policy | 115k files, error rows not panics | P1 |
| tag read: ID3v1/2.2–2.4, Vorbis, APE, MP4 | `lofty`; multi-value fields kept | P1 |
| album grouping, Various Artists, albumartist fallback | the #1 source of wrong library views | P1 |
| multi-disc, disc totals, track/disc ordering | `1/12`, `1-01` filename forms | P1 |
| cover art cache + fallbacks | above | P1 |
| **m3u/m3u8/pls import** (1,654 on disk) | paths resolved into the index, missing rows kept | P1 |
| **cue sheets** (363 on disk) | single-file albums become virtual tracks with offsets | P1 read / P3 write |
| views: filter + columns + sort + group-by, saved, bindable | the foobar layout primitive | P1 |
| quick filter (nucleo) + FTS search | open-ended: instant over the current view | P1 |
| ratings, play/skip counts, last played, "recently added" | cheap, enables autoplaylists | P1 |
| **tag edit**: multi-file, per-field dirty tracking, atomic (temp+rename), undo | one write per gesture; unchanged fields untouched | P2 |
| **file ops**: rename/move/organize by pattern, dry-run preview, undo journal | foobar's file-rename culture; template like `%albumartist%/%album%/%track%. %title%` | P2 |
| **missing-file detection + relocate** | match by filename/size/duration after a move | P2 |
| **duplicate finder** | (artist, album, disc, track) + duration/size/head-hash | P2 |
| **query language subset** | `artist IS "x" AND year > 1990 AND %path% HAS live` | P2 |
| **autoplaylists** | a saved query that is live: "added this week", "top rated", "never played" | P2 |
| ReplayGain scan + tag write (track & album) | R128 fallback; playback gain = one multiply | P2 |
| tag ⇄ filename guessing (`%track% - %artist% - %title%`) | both directions, previewed | P2 |
| art write/extract, per-album override | embedded or sidecar | P2 |
| tech **Properties** panel | every tag field, file facts, gain, peaks, art | P2 |
| playlist export (m3u/m3u8, relative paths) | round-trip with import | P2 |
| library stats (counts, durations, codecs, top artists) | trivial once the DB is right | P2 |

Non-goals kept out on purpose: streaming, transcoding, plugin ABI, CD, scrobbling, tag-from-acoustid,
audio fingerprinting beyond a head-hash heuristic.

---

## 6. Panel vocabulary

Panels are the leaves of a layout (§3). Each publishes cells + `Overlay`, and degrades by size class.

| Panel | Cell treatment | Bitmap content |
|---|---|---|
| `tree` | artist → album → track, or folder tree; album detail header | album art (half-blocks) |
| `table` | virtualised, sortable, multi-select, configurable columns, group headers | — |
| `albums` | album wall as cells (title + year + count) | **art grid**, real covers |
| `playlist` | ordered, now-playing marker, queue vs current tabs | — |
| `nowplaying` | title/artist/album/codec/sr/bitrate/RG/tech, big art block | **art**, **waveform**, **meters** |
| `waveform` | min/max band per cell + playhead | 1 px per peak, seek cursor |
| `spectrum` | block columns | overlay bands (the window showcase) |
| `scope` | block columns (oscilloscope) | polyline overlay |
| `meters` | per-channel bar rows | thin quads |
| `tech` | the Properties panel: all fields, file facts | art preview |
| `search` | query input + live result count | — |
| `console` | job log / errors / scan progress | — |

Default layout is fooyin's shape: `tree | (table ⇄ albums tabs) | (nowplaying / playlist)`, one key to
full-width art, one to full-width spectrum. Keyboard-first with two binding sets (foobar-ish arrows,
plus vim), `?` for the live map, `Space` = play/pause everywhere, `/` = filter, `:` = command/query,
`q` = queue, `e` = play next, `Alt+1..9` = workspace, `Ctrl+L` = layout mode.

---

## 7. Audio path

```
decode thread ─► resample ─► gain(RG) ─► fade ─► SPSC ring ─► cpal callback
 (symphonia)     (rubato)     (f32)              (ringbuf)    (no alloc, no lock)
      └──► peaks (lazy) ──► DB + Overlay bands
```

Formats: mp3, flac, wav, ogg vorbis, aac/m4a, alac. **Opus: indexed, listed, unplayable** ("needs
libopus") — a visible, honest row beats a silently missing album.

- **Gapless from P0.** The next track's decoder opens and pre-rolls before the current one ends and the
  handoff happens inside the ring. Retrofitting gapless costs more than building it in, and a player
  that clicks between tracks of a gapless album has failed the format's one promise.
- **Seek** = decoder restart at a frame offset + a 5 ms gain ramp, verified by a "no DC step" test on
  the rendered buffer.
- **Volume** = RG gain × user gain in f32; no DSP in the callback. Crossfade (P3) is the same fade
  machinery with two prepared decoders.
- **Spectrum** = `rustfft` on a copy of the last block, taken in the renderer thread from a small
  lock-free tap, never in the callback.
- Non-goal: bit-perfect. We resample to the device rate and say so in the tech panel.

---

## 8. Headless surface (single write path)

`fbp-tools` is the only mutator. The TUI dispatches into it and these mount on top:

**CLI** (`mpc`-shaped, for tmux/waybar and for scripted repair):
`fbp scan`, `fbp search <q> [--json]`, `fbp play <q|id>`, `fbp pause|next|prev`, `fbp now`,
`fbp queue <q>`, `fbp art <id> --out f.png`, `fbp tag set --query <q> --field artist=X`,
`fbp file move --query <q> --pattern '...' --dry-run`, `fbp relocate`, `fbp dupes`, `fbp stats`.

**MCP**: `library.search/track/stats`, `scan.start/status`, `playlist.*`, `view.*`, `player.*`,
`tags.get/set`, `files.move/relocate/dupes`, `layout.get/set`, `art.get`, `shots.capture`.

Discipline payoff: agent-driveable, scriptable, and the TUI's own tests become handler tests. A panel
that mutates state directly is a bug, not a style choice.

---

## 9. Persistence & config

- `library.db` (SQLite, WAL) in the XDG data dir; portable mode via `--data-dir`.
- `config.toml`: library roots, art mode, keymap set, theme, device, `gain_mode`, fade ms, resampler
  quality, default layout.
- `layouts/<name>.toml`: the layout tree (§3), human-editable, shareable.
- Session state (`meta`/`ui`): last workspace, focus, scroll, selected view — a restart lands where
  you left.
- Playlists are DB-native (exportable to m3u/m3u8); cue sheets import as a playlist of virtual tracks.

---

## 10. Phases and acceptance

Every phase ends green on `cargo test`, `cargo build --no-default-features` (headless: no ALSA, no
wgpu), and `fbp --shots out/` for every panel and every layout in `layouts/` at 80×24 and 200×60.

**P0 — the walk (audio proves itself).** `--root` walk, no index: list files, play/pause/next/prev/
volume through cpal, gapless auto-advance, now-playing line.
*Accept:* a 60-minute mp3 plays end-to-end with **zero-sample** gap on auto-advance; pause/resume
< 50 ms; `--no-default-features` builds; decode→ring→out pinned by a sample hash.

**Landed 2026-10-29** (`crates/fbp-audio`, `crates/fbp`). A playlist is one continuous
sample stream: `Sequence` opens the next file the moment the current one ends, so
`the_playlist_is_the_exact_concatenation_of_its_files` asserts the output equals the
sample concatenation of the inputs — no silence inserted, none dropped. Pause is
silence *without consuming* the queue (instant resume), and a skip bumps a generation
counter so blocks already queued for the old track are discarded rather than heard late.
Resampling is per track and only on a rate mismatch (same-rate material is bit-exact);
the interpolator's group delay is dropped once per track so a converted track is not
shifted ~1.5 ms against an unconverted one. `Sequence` skips unopenable files as error
rows and keeps playing. Evidence on the real machine: mp3/flac/wav decode; **opus is
probed, listed, and reported as unsupported** (D3 as designed); the 115k-file walk (100,640
audio files) takes **465 ms** in a debug build; all 13 tests pass with and without the
`device` feature. Two committed 2 s LAME mp3 fixtures pin the other half of gapless —
`mp3_encoder_delay_is_trimmed` (the trim flag is on) and `mp3_tracks_join_without_silence`
(no silent block at the seam).

**P1a — the index (landed 2026-10-29).** SQLite schema + migrations (v2), the incremental scanner
(parallel readers, single writer, one bulk diff, in-memory removal detection), `lofty` tags without
cover art, FTS5 search, album/artist rollups, stats, and the `scan`/`search`/`stats`/`play` verbs on
top of them. 24 tests, including a serial-vs-parallel determinism check. Numbers are in §5: a warm
rescan of 100,640 files is **0.5 s** (budget < 5 s), FTS is 3–21 ms, and the real library's 102
unparseable files are visible error rows.

**P1b — the library you can look at (landed 2026-10-29).** The layout engine (panel/tabs/split, TOML,
in-app editing, cell-granular weights); five panels (library tree, track table, album wall, now playing,
console); the async art cache with half-block projection; the FTS quick filter; playback from the
table/album wall; a background scan with progress in the console; session restore (layout, focus,
filter, sort); and `--shots`, which renders any frame to a PNG through `TestBackend` + `font8x8`.

*Evidence:* 37 tests, including the two that matter here — `the_albums_panel_renders_real_cover_art`
(scans a fixture library with a real cover, renders the albums tab, decodes the PNG and requires >200
distinct colours, i.e. the art reached the pixels) and
`the_default_layout_round_trips_through_toml` / `split_close_and_resize_keep_the_tree_well_formed`
(the tree survives editing). Live checks: the TUI runs in a pty and quits cleanly on `q`; four PNGs
(80×24, 200×60, and the albums tab at both) render from the real 100,640-track library; the whole UI
also builds and renders with `--no-default-features` (no audio stack at all), which is what makes
`--shots` usable in CI.

**Moved out of P1b into P1c:** MCP, m3u/cue import, playback stats (play counts/ratings UI), and
panels bound to saved views. Also deferred, with the reason recorded: the quick filter uses FTS5
(3–18 ms in-process) rather than `nucleo` — it meets the keystroke budget on the real library, so the
fuzzy matcher only earns its place for in-view filtering, which is P1c. Cover art is keyed by album
(`albumartist \x1f album`) rather than by content hash: it is fetched lazily per album, so the album
key *is* the dedupe, and the content hash buys nothing until art is written back (P2).

**P2 — library management (the weight centre).** Tag write; file ops (move/rename/organize, dry-run,
undo journal); missing-file relocate; duplicate finder; query language + autoplaylists; ReplayGain
scan/write; tag⇄filename guessing; art write; Properties panel; playlist export.
*Accept:* a multi-file tag edit is **one** undo entry and survives a kill mid-write (temp+rename, no
partial file); a 500-file rename+move batch is fully undone by one action; relocate finds a moved album
by filename+size+duration with no false positives in the fixture set; a 20-case query corpus round-trips
against FTS; m3u exported by fbp re-imports identically.

**P3 — pixels and polish.** `--window` becomes the **default** (`--tui` opts out); `scene.rs` overlays
for waveform, spectrum, scope and meters; kitty/sixel art; full-screen spectrum; crossfade; EQ panel;
MPRIS; layout editor's graphical niceties (drag-free, still key-driven).
*Accept:* the same layout renders in both front-ends with identical text and identical geometry (a
shared-geometry test asserts the cell bands and the overlay bands agree within one pixel); the wgpu
offscreen pin paints bands and a snapped playhead; idle CPU < 5 % of a core at 20 fps.

**Later / only if asked:** opus (needs libopus), transcoding, DSP chain, CD, scrobbling, plugin panels.

Budget table (release build, the real 115k library):

| Metric | Target | Measured |
|---|---|---|
| cold start → interactive (warm DB) | < 150 ms | 26 ms empty DB; **591 ms** for the shot path on 100k tracks (16 covers fetched + six 3200×1200 PNGs), of which the row/album/artist load is a fraction |
| warm rescan (stat only) | < 5 s | **0.5–0.7 s** (P1a) |
| first-ever scan of 100k files | — | ~40 min serial, ~17 min with 8 readers (this RAID1) |
| key → repaint | < 16 ms | — |
| filter over 100k rows | < 16 ms/keystroke | **3–18 ms** in-process (3–21 ms incl. process start) |
| seek → audible | < 100 ms | — |
| track → track (gapless) | 0 samples | **0** (P0) |
| RSS with 100k tracks + art | < 300 MB | — |
| idle CPU (playing, terminal) | < 2 % | — |
| overlay frame (window, 20 fps) | < 5 % of a core | — |

---

## 11. Testing

- **Handler tests are the suite.** Everything goes through `fbp-tools`, so UI + CLI + MCP share coverage.
- **Deterministic audio**: render N samples, hash; seek/ramp asserts no DC step > 1e-3; gapless asserts
  the concatenation boundary is sample-continuous.
- **Fixtures**: `fbp-fixtures` generates a real tiny library (6 albums, valid encoders, unicode and
  200-char paths, missing tags, a cue sheet, a multi-disc set, duplicate covers, a truncated mp3) so no
  test depends on the machine's 115k files.
- **Filesystem tests** for the dangerous paths only (tag write, move/organize, relocate) — those are the
  ones that lose data; they run against a temp dir with an injected kill-mid-write.
- **Visual**: `--shots` per panel per layout + structural wgpu offscreen pins (bands, snapped marker) —
  structural, not pixel-golden, because rasterisers differ.
- **Perf guards**: a synthetic 100k-row DB bench that fails on filter/sort regression.

---

## 12. Decisions

| # | Question | Decision |
|---|---|---|
| D1 | Name / binary | `fbp` for now; rename is cheap today, expensive later |
| D2 | Default front-end | terminal default **now**, window default **at P3**, `--tui`/--window` both permanent. Built as peers from day one so the flip is a flag, not a port |
| D3 | Opus | **not in v1**: indexed, listed as needing libopus. No decoder written |
| D4 | Platforms | **Linux only** (cpal portable as a side effect; no exclusive mode) |
| D5 | Art baseline | half-block truecolor; kitty/sixel at P3; 256-colour quantised is a fallback, never the design |
| D6 | Index | SQLite (bundled, WAL, FTS5). No bespoke index |
| D7 | Gapless | in from P0 |
| D8 | Layout flexibility | 3 node kinds (panel/tabs/split), fixed panel vocabulary, TOML files, depth ≤ 4. **No** scripting or UI DSL. Slightly-less-flexible-than-foobar is the goal, not a compromise |
| D9 | Tag writing | P2, atomic, undoable, multi-file. Reading is P1 |
| D10 | `fbp-tools` as MCP host | yes from P1, thin: handler signatures + serde |

---

## 13. Risks

1. **Table performance in `ratatui`.** Building every visible line each frame is fine; building 100k is
   not. Only the visible slice of an `Arc<Vec<RowId>>` is ever materialised — a P1 acceptance test, not
   a later optimisation.
2. **Tag writing and file moves are the data-loss surface.** They are the only features that get
   temp+rename, dry-run previews, an undo journal and kill-mid-write tests. Everything else is safe
   because it is only a DB row.
3. **Library correctness is unglamorous and endless**: albumartist fallbacks, compilations, multi-disc,
   disc totals, unicode normalisation, cue offsets, art priority. Budget real time; the fixture library
   is the tool that keeps it honest. *First taste in P1a: 102 of 100,640 real files fail to parse, the
   error list was wiped by the first warm rescan (now keyed by path), and \"albumartist else artist\" is a
   grouping rule that the real data already leans on.*
4. **Art in terminals is a compatibility swamp** (kitty detection, sixel, tmux passthrough, truecolor).
   Half-blocks are the floor and always work; everything else is a toggle with a visible fallback.
5. **Two front-ends drift.** Mitigated only by invariant 3 (one input path, one tick, one render); the
   shared-geometry test in P3 is the tripwire.
6. **Layout breadth is a tar pit.** foobar can do anything and pays for it in a decade of edge cases;
   the three-node vocabulary is the guardrail. Any request that needs a fourth node kind is a request
   for a different product.
7. **Scope creep toward a DAW or a file manager.** Anything with a timeline is out; anything that
   rewrites files needs a dry run.
