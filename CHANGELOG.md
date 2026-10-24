# Changelog

## Unreleased

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
