# Changelog

## 0.1.8 — 2026-11-09

- `engram.log` rotates at 1 MiB to `engram.log.1` (one old file kept), so the log stays under
  ~2 MiB however long engram runs.

## 0.1.7 — 2026-11-08

Security fixes from an internal audit. **Upgrade every device**: peers on 0.1.7 skip manifests
from older versions until those devices upgrade (each republishes once automatically).

- Manifest keys that are not plain relative paths (`..`, absolute, `./`, `//`, control chars),
  keys under a symlinked directory, and case-insensitive duplicates are ignored on pull. A peer
  or forged manifest could otherwise write or delete files outside the synced directory.
- Manifests now carry their collection; a manifest must match its device and collection prefix
  and have a higher `seq` than the last one accepted. A bucket writer without the identity can no
  longer replay old manifests (rollback, resurrected or deleted files) or copy one under a new
  device name. `seq` has a wall-clock floor so a lost `state.json` cannot freeze a device.
- Blobs fetched by key are checked against their sha256 after decrypt.
- Device names from the bucket listing must match `[A-Za-z0-9._-]{1,64}`, so a crafted prefix
  cannot put text into the agent context or the log. Same check for plugin lists.
- A conflict copy never replaces an earlier one (`name.conflict-<dev>-2.md`).
- One bad device is skipped with a warning (always shown, even with timings off) instead of
  failing the whole pull. Remote objects over 256 MiB are refused; mtimes are clamped.
- Data-loss fixes: `collection remove` forgets that collection's sync state, `init` moves an old
  `state.json` aside, and a collection whose directory is missing is not pushed, so none of these
  can push tombstones that wipe peers. New `engram device remove <name>` deletes a retired
  device's data so its old files stop coming back.
- Detection: each pull checks this device's own manifest on the remote. If someone else
  overwrote or deleted it, engram warns and republishes it. A new device name, unsafe keys
  dropped from a manifest, and skipped devices each print one warning line. The CLI no longer
  prints `up to date` next to a pull error.
- `engram device remove` also adds the device to `ignored_devices`, so a token it still holds
  cannot bring its data back. `SECURITY.md` documents the warning signs and the recovery steps:
  rotate the token and the identity together.
- Hardening: `~/.engram` is 0700, log/inbox 0600, state writes use fresh `O_EXCL` temp files, the
  hook command quotes the binary path, CLI pull/push take the hook lock, `collection add` of a
  `.claude` directory excludes `settings*.json`, the launcher requires the checksum line to name
  its tarball, and releases build with `--locked` after `cargo test`.

## 0.1.6 — 2026-11-08

- Guided setup: `/engram:setup` skill (R2 bucket and token, first or extra device, keys never
  pasted into the chat). Session start says `engram: not set up yet` until `engram init` runs.
- `engram init --plugin` also sets `autoMemoryDirectory`. Flags read `ENGRAM_REMOTE`,
  `ENGRAM_ENDPOINT`, `ENGRAM_ACCESS_KEY` and `ENGRAM_SECRET_KEY`. `ENGRAM_IDENTITY` joins an
  existing remote without copying `identity.txt`.
- The launcher links the current release as `~/.engram/bin/engram` for use in terminals.

## 0.1.5 — 2026-11-08

- Plugin for Claude Code and Codex (`plugin/`, marketplace at the repo root): hooks, an `engram`
  skill and a launcher that downloads and sha256-checks the pinned release on first use.
  `engram install-hooks --plugin` sets only `autoMemoryDirectory` and drops old engram hooks.
- Codex support: session start hands Codex the memory dir and `MEMORY.md` (no
  `autoMemoryDirectory` there); `apply_patch` edits are pushed like Edit/Write.
- Hook timings are off by default; `engram hook-timing on|off` toggles them.
- Hooks read stdin with a 300 ms cap, so a caller that never closes stdin can't hang them.

## 0.1.4 — 2026-11-05

- Prompt hook pulls in a detached background process (`"background_pull"`, default on) and
  returns right away: 327 ms → <1 ms of engram work (process start ~14 ms). Results go to
  `~/.engram/inbox/` and reach Claude on the next prompt; files land on disk ~0.3 s after the
  prompt. SessionStart still pulls synchronously.

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
