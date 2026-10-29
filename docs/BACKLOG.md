# Backlog (the things a phase deliberately left out)

A phase is only finished if what it dropped is written down. This file is the ledger; SCOPE.md is
the plan. Nothing here is forgotten, only deferred.

## P1c — the small stuff (deferred out of P1b, still owed)

- [ ] **MCP server** over the handler layer: `library.search/track/stats`, `scan.start/status`,
      `player.*`, `playlist.*`, `art.get`, `views.*`, `tags.get/set`, `shots.capture`.
      (D10 says "thin: handler signatures + serde". The TUI is already the single-writer client,
      so this is mostly plumbing — but it must not be skipped, it is the agent/automation surface.)
- [ ] **m3u/m3u8/pls import** (1,654 playlists sit in the real library) and export.
- [ ] **Cue sheets** (363 in the real library): single-file albums become offset tracks.
- [ ] **Playlist panel** as a real panel: ordered, reorderable, persistent, plus a queue tab.
- [ ] **Saved views bound to panels**: `views(id, name, filter, columns, sort, group_by)` rows, a
      panel instance carrying a view id (today: one instance per panel kind).
- [ ] **Play counts / ratings / last-played / skip counts**: columns exist and are unused; needs the
      write path from playback and a star column in the table.
- [ ] **Mouse**: hit-test on the layout rects (invariant 2), click to select/play.
- [ ] **Command palette** (`:`) over the handler verbs.
- [ ] **nucleo** in-view fuzzy filter (the FTS filter measured 3–18 ms, which is why this is deferred,
      not dropped: fuzzy is for narrowing *within* a view).
- [ ] **256-colour art fallback** (half-block colour quantisation when `COLORTERM` is absent).
- [ ] **Tab bar polish**: the tab strip is one line of text today.

## P2 — library management (the weight centre), in slices

- [x] **P2a tag write** (landed 2026-10-29): patch-based multi-file writing, atomic temp+rename,
      journal-before-write, undo, the Properties panel with inline editing and apply-to-all.
      *Still owed from this slice:* a multi-select model (today: one track + "apply to filtered"),
      and an in-place writer with ID3v2 padding as the fast path.
- [ ] **P2b file ops**: rename/move/organize by pattern with dry-run preview, sharing the P2a
      journal; plus `fbp tag set --dry-run` on the CLI (the scriptable half of P2a).
- [ ] **P2c repair & query**: missing-file relocate (filename+size+duration), duplicate finder,
      query language subset, autoplaylists.
- [ ] **P2d totals**: ReplayGain scan + tag write, tag⇄filename guessing, art write/extract,
      playlist export, library stats panel.

## Later (explicitly not now)

- Opus decoding (needs libopus), transcoding, DSP chain/EQ, CD, scrobbling, plugin panels,
  windowed pixel front-end + scene overlay (P3).
