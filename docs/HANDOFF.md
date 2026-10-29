# Handoff — where fbp is, and how to pick it up

**Last updated:** 2026-10-29 · **branch:** `main` · **tests:** 53 · **size:** ~7.6k lines of Rust

Read in this order: this file (state) → [SCOPE.md](SCOPE.md) (the plan and the decisions) →
[BACKLOG.md](BACKLOG.md) (what each phase deliberately dropped).

---

## TL;DR

A TUI library player that works on a real 100,640-track library. Four slices are done:

| slice | what it proves | state |
|---|---|---|
| **P0** audio path | a playlist is one continuous sample stream; gapless is exact | done |
| **P1a** index | a warm rescan of 100k files is 0.5 s; FTS is 3–18 ms | done |
| **P1b** UI | layout tree, 6 panels, half-block cover art, deterministic PNG shots | done |
| **P2a** tag writing | atomic, journalled, undoable, driven from a Properties panel | done |
| **P2b** file ops | move/rename/organize, relocate, dupes, `tag set --dry-run` | **next** |
| P1c small stuff | MCP, m3u/cue, playlist panel, saved views, counts/ratings, mouse | owed — see BACKLOG |

---

## Run it

```bash
cargo run --release                                    # the TUI (uses ~/.local/share/fbp/library.db)
cargo run --release -- scan --root ~/Music             # index a folder
cargo run --release -- stats                           # counts, codecs, hours
cargo run --release -- search bjork                    # FTS from the shell
cargo run --release --features shots -- --shots shots  # render the panels to PNGs
cargo build --no-default-features                      # no ALSA, no wgpu: the UI still runs
```

Keys: `/` filter · `Tab` focus · `jk` move · `Enter` play (or edit a tag field) · `space` pause ·
`n`/`p` next/prev · `s` sort · `S` scan · `m`/`M` tab stacks · `u` undo · `A` apply field to filtered ·
`Ctrl+L` layout mode · `Ctrl+S` save layout · `Alt+1..9` load slot · `q` quit.

On this machine the library already lives at `~/.local/share/fbp/library.db` (100,640 tracks, indexed
from `/mnt/md0/mp3`).

---

## The map

```
crates/
  fbp-core     (269)   Track, is_audio, tag::{Field, Patch}. No I/O, no threads.
  fbp-audio    (1241)  decode (symphonia) → resample (rubato) → queue → cpal; the gapless Sequence.
  fbp-index    (2413)  SQLite schema, incremental scanner, FTS queries, album art cache, tag writer
                       + undo journal.
  fbp          (3663)  the app: config, CLI verbs, and tui/ (layout, panels, artcache, playback,
                       blocks = half-block projector, shot = TestBackend→PNG, theme).
```

### How the pieces fit

- **Audio (P0).** `Sequence` is handed the playlist and returns "the next block", opening the next file
the moment the current one ends — so same-rate material is the *exact sample concatenation* of the
files. Producer thread feeds a bounded queue; `fill_output` is the only place blocks become samples
(pause = silence without consuming; skip = a generation counter drops stale blocks).
- **Index (P1a).** One walk → one bulk `SELECT` of the root's rows → metadata pass → tag reads only for
files whose `(size, mtime, inode)` changed → 8 parallel readers, one writer, 512 rows per transaction →
removals from an in-memory `seen` set. Cover art is *not* parsed at scan time.
- **UI (P1b).** A TOML tree of `panel`/`tabs`/`split` nodes. `split(rect, axis, weights)` is the only
geometry; panels get a rect and draw. Every list renders only its visible slice. `--shots` renders the
same frames through `TestBackend` + `font8x8` to PNG, which is how the art and layout get reviewed.
- **Art.** Half blocks: one cell is two stacked pixels (`▀`, fg over bg), so a 40×20 tile is a 40×40
truecolor cover. Fetched off the render path by a worker thread, thumbnailed to 96 px, cached in SQLite,
LRU'd at 64 in memory.
- **Tag writing (P2a).** `Patch` = fields somebody actually edited. Every file is copied beside itself,
edited, fsynced and renamed over the original. The undo journal row is written *before* the first file
is touched, so a crash mid-batch is still undoable. One gesture = one row, files and index both restored.

---

## Verification recipe

```bash
cargo test --features shots      # 53 tests; the shot tests decode their own PNGs
cargo clippy --workspace --all-targets --features fbp/shots   # must stay at zero warnings
cargo build --no-default-features                             # the headless contract
rm -rf shots && XDG_CONFIG_HOME=/tmp/x ./target/release/fbp --shots shots   # deterministic images
```

What the interesting tests actually assert:

| test | property |
|---|---|
| `the_playlist_is_the_exact_concatenation_of_its_files` | gapless is *exact*, not approximate |
| `mp3_encoder_delay_is_trimmed` | 2 s of mp3 decodes to 2 s, not 2 s + 1105 samples |
| `a_rescan_of_unchanged_files_writes_nothing` | a warm scan is a stat pass |
| `the_albums_panel_renders_real_cover_art` | the cover's colours reach the PNG (>200 distinct) |
| `the_tab_stack_changes_what_is_captured` | a capture reflects the tab, not the session behind it |
| `one_gesture_over_many_files_is_one_undo_entry` | undo is per gesture |
| `a_write_that_cannot_happen_leaves_the_original_bytes_alone` | a failed write touches nothing |
| `apply_to_all_asks_before_it_writes_the_filtered_set` | real keys → file on disk, y/N honoured |

---

## Measured envelope (real library, RAID1 of two spinning disks)

| | |
|---|---|
| indexed | 100,640 files → 18,181 albums, 11,931 artists, 447 d of audio, 1,061 GiB, 78 MB DB |
| warm rescan | **0.5–0.7 s** |
| first-ever scan | IO-bound: ~40 min serial, ~17 min with 8 readers (~2.3×) |
| FTS query | 3–18 ms in-process |
| cold start | 26 ms empty DB; ~0.6 s for the shot path (16 covers + six 3200×1200 PNGs) |
| tag write | a rewrite per file (atomic); undo re-encodes the tag block, ~1 KB of padding |

---

## Sharp edges (each one cost a debugging session)

1. **A temp file must keep the real extension.** lofty picks the format from the extension; `.tmp` made
every tag write fail with "failed to parse file".
2. **`--shots` must not inherit a session.** Two "different" captures came out byte-identical because
the restored session had one tab stack instead of two. `reset_for_capture` + a tab-switch test.
3. **Integer columns are integers.** Reading `year`/`track_no` as `Option<String>` is how a tag editor
crashes on its own journal.
4. **Layout resize arithmetic**: a negative delta cast to `u16` underflowed on the second arrow press.
5. **Rust ignores SIGPIPE**, so `fbp search x | head` panicked instead of exiting. `libc::signal` in main.
6. **A pty hands you a NUL** (Ctrl+Space) at startup; a key handler that ignores modifiers pauses the
transport on launch.
7. **A pty with no size reports 0 columns**; never truncate a status line to nothing.
8. **The `errors` table is evidence, not a per-scan scratch pad.** Keyed by path, it survives warm
rescans; a file that reads cleanly clears its own row.
9. **Page cache lies about concurrency.** A 1-thread vs 8-thread scan on the second run measured 170× —
because the first run had warmed the cache. The honest number is the cold one (~2.3×).

---

## Next: P2b — file operations

Plan, in the order it should be built:

1. **`fbp-files` crate** (or `fbp-index::files`): `organize(pattern)` → a `Plan` of `(from, to)` moves
   computed from tags, rendered by the CLI as a table and by the panel as a preview. Nothing moves until
   `confirm`.
2. **Pattern language**: `%albumartist%/%album%/%track%. %title%` with a documented token set
   (`%albumartist% %artist% %album% %track% %disc% %title% %year% %genre%`), plus sanitising for `/` and
   characters Windows-hostile even though we are Linux-only (a library is portable data).
3. **The journal** already exists: extend `undo.kind` to `files` with a payload of `(from, to)` pairs,
   written before the first rename, applied in reverse on undo.
4. **Collision policy**: refuse to overwrite; report the batch's collisions in the preview (`-> exists`).
5. **Relocate**: for rows with `missing = 1`, search the root for a file whose name matches and whose
   size (and duration, from the tag) match; propose, never auto-apply.
6. **Duplicates**: group by dims `(albumartist, album, disc, track)` and by `(duration, size, first-N-bytes
   hash)`; report, then offer delete-to-trash as a *separate* confirmed action (and it is the one place
   a `fs::remove` will ever appear).
7. **`fbp tag set --field artist=X --query "..." --dry-run`** — the CLI half of P2a, same writer.

Acceptance for the slice is already written in SCOPE §10 (500-file rename undone by one action; no
false positives in the relocate fixture; m3u export re-imports identically).

## Open questions for the next session

- Multi-select in the table (needed before P2b's delete-to-trash feels safe) or batch-by-filter only?
- Should `organize` move the *directory* or just rename files inside it, when several files map to the
  same target album folder?
- Undo depth: keep unbounded (a row per gesture, tiny) or cap and prune?
