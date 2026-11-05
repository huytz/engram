# Changelog

## Unreleased

## 0.1.3 — 2026-11-05

- Prompt hook reuses the device list for `device_list_ttl` seconds (default 600) instead of a
  LIST per prompt; SessionStart and `engram pull` still LIST. Prompt hook p50 349 → 327 ms.
- Manifests carry the content of blobs new in that push (`inline`, up to 256 KB), so a reader
  that is caught up skips the blob GET: pull 1 file 544 → 395 ms, pull 20 files 896 → 580 ms.
  Inlined blobs are PUT alongside the manifest: push 1 file 780 → 649 ms. Blobs are still
  uploaded, so readers further behind and v0.1.2 fall back to blob GETs.

## 0.1.2 — 2026-11-05

- Pull fetches all collections and devices concurrently; the device LIST runs alongside the
  conditional GETs of already-known devices, so a quiet pull is one round trip. Blobs are
  fetched in parallel (16 in flight).
- Push drops the per-blob HEAD (a sha already in our last manifest is already uploaded, PUT is
  idempotent), uploads blobs in parallel and pushes collections and the plugin list together.
- `state.json` is written once per run instead of once per device.
- Static keys + endpoint build the S3 client directly, skipping the default provider chain;
  single-threaded runtime.
- OS TLS roots are cached in `~/.engram/roots.pem` for a day: reading the macOS keychain cost
  ~70 ms on every run. Local overhead per hook ~80 ms → ~3 ms.
- Hooks report per-action timings in Claude Code (`systemMessage`), e.g.
  `engram pull 412ms · memory 398ms (up to date)`. Turn off with `"hook_timing": false` in
  `config.json`. Pulled index lines now go to Claude as `additionalContext`.
- `scripts/bench.py BIN [BIN...]` benchmarks binaries against your remote (scratch collection, removed after).

## 0.1.1 — 2026-11-04

- Per-prompt pull lists only device prefixes (S3 delimiter) instead of every blob key, and pulls
  only the `memory` collection; other collections still pull on SessionStart. ~3.5s → ~0.6s on R2.
- Per-prompt pull skips after ~1s when a push holds the lock, instead of waiting past the 5s hook
  timeout.
- Push reuses the previous sha when size and mtime are unchanged, so only changed files are read
  and hashed.

## 0.1.0 — 2026-10-24

First release.

- Sync Claude Code auto-memory and local skills through any S3-compatible bucket
  (Cloudflare R2, AWS S3, MinIO) or a `file://` directory.
- One writer per `<collection>/<device>/` prefix; content-addressed, age-encrypted blobs;
  manifest as the atomic commit point; tombstones for deletions.
- `MEMORY.md` merge: adopt the newer index when the local one is unchanged, otherwise a
  line-level union keyed by file link. Other conflicts: newest wins, loser kept as
  `name.conflict-<device>.ext`.
- Claude Code hooks: pull on SessionStart and on every prompt (one conditional GET), push after
  Edit/Write of a memory or skill file and on Stop. New index lines are surfaced to Claude
  mid-session.
- `engram install-hooks` / `uninstall-hooks` edit `~/.claude/settings.json` in place and set
  `autoMemoryDirectory`.
- `*.conflict-*` copies, `.DS_Store` and `*.tmp` are never uploaded.
- `engram collection add|remove|list` to sync any directory; `engram plugins` to diff installed
  Claude Code plugins across devices (lists are published on every push).
