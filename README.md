# engram

**One Claude Code memory for all your devices.**

Claude Code remembers what it learns with you (auto-memory: `MEMORY.md` + topic files, plus your
skills), but only on the machine where it learned it. engram syncs those directories through a
bucket you own (Cloudflare R2, S3, MinIO), age-encrypted before upload, so every device works
from the same memory.

```
   device 1                        R2                        device 2
┌──────────────┐   push ──▶   ┌─────────────┐   ◀── push   ┌──────────────┐
│ memory       │              │ memory/<d>  │              │ memory       │
│ skills       │   ◀── pull   │ skills/<d>  │   pull ──▶   │ skills       │
│ plans, rules │              │ <any>/<d>   │              │ plans, rules │
└──────────────┘              └─────────────┘              └──────────────┘
```

## How one memory works

- **One directory.** `autoMemoryDirectory` points every project at `~/.engram/memory`, so all
  sessions on a device read and write the same memory.
- **Push on save.** After Claude edits a memory or skill, a hook uploads only the changed files
  into that device's own prefix, then a manifest that commits them. No server, no locks: each
  device writes only its prefix, so nothing is overwritten.
- **Pull on every prompt, off the critical path.** Each prompt starts a background pull (one
  conditional GET per device, in parallel) and returns in a few ms. New files land on disk ~0.3 s
  later; new `MEMORY.md` lines are handed to Claude on the next prompt. Session start pulls
  everything synchronously so `MEMORY.md` is fresh when Claude loads it.
- **Merge, never lose.** `MEMORY.md` is merged line by line; other conflicts keep the newer file
  and save the other as `name.conflict-<device>.md`. Deletions propagate.

`engram hook-timing on` makes each hook report its cost, e.g. `engram pull 412ms · memory 398ms (up to date)`.

## Install

As a plugin for Claude Code and Codex (hooks + an `engram` skill; the release binary is fetched
and sha256-checked on first use):

```sh
claude plugin marketplace add huytz/engram && claude plugin install engram@engram
codex plugin marketplace add huytz/engram && codex plugin add engram@engram
```

Then `engram init …` and `engram install-hooks --plugin` once per device; in Codex, trust the
engram hooks when it asks. Codex has no `autoMemoryDirectory`, so engram hands it `MEMORY.md` at
session start. A plain binary install also works. Bucket setup, commands and options:
[docs/setup.md](docs/setup.md). Unofficial; built only on documented features
(`autoMemoryDirectory`, hooks, plugins).

## Benchmark

`scripts/bench.py target/release/engram`, v0.1.4, 10 rounds (prompt hook 20), 2026-11-05,
Apple M5 / 16 GB, macOS, home network to Cloudflare R2 (~150 ms RTT). Real data: 5 collections,
~1,100 files; write cases use a scratch collection of 400 × 5 KB files.

| scenario | p50 | p90 |
|---|---:|---:|
| prompt hook (background pull, default) | <1 ms | <1 ms |
| prompt hook (`"background_pull": false`) | 327 ms | 396 ms |
| full pull, 5 collections (SessionStart) | 413 ms | 518 ms |
| push, nothing changed | 23 ms | 27 ms |
| push 1 file | 649 ms | 709 ms |
| pull 1 file | 395 ms | 527 ms |
| push 20 files | 879 ms | 1407 ms |
| pull 20 files | 580 ms | 1303 ms |
| first push, 400 files | 8.6 s | |
| first pull, 400 files | 5.3 s | |

Hook times are engram's own work as it reports them; process start adds ~14 ms, the same as
`/usr/bin/true`. A pull costs one round trip to R2 (TCP + TLS + ~120–200 ms of R2 time from
Singapore to an APAC bucket), the floor for a fresh process, so the prompt hook no longer waits
for it. Pulling a small change is one round trip too: new content rides inline in the manifest.
Push hooks run async, so they never delay Claude.

[CONTRIBUTING](CONTRIBUTING.md) · [SECURITY](SECURITY.md) · [MIT](LICENSE)
