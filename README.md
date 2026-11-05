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
- **Pull on every prompt.** Before each prompt a hook asks R2 "did any other device change?" with
  one conditional GET per device, in parallel. New files land on disk and new `MEMORY.md` lines
  are handed to Claude in that same prompt.
- **Merge, never lose.** `MEMORY.md` is merged line by line; other conflicts keep the newer file
  and save the other as `name.conflict-<device>.md`. Deletions propagate.

Each hook reports its cost in Claude Code, e.g. `engram pull 412ms · memory 398ms (up to date)`.

Install, setup and commands: [docs/setup.md](docs/setup.md). Unofficial; built only on documented
Claude Code features (`autoMemoryDirectory`, hooks).

## Benchmark

`scripts/bench.py target/release/engram`, v0.1.3, 10 rounds (prompt hook 20), 2026-11-05,
Apple M5 / 16 GB, macOS, home network to Cloudflare R2 (~150 ms RTT). Real data: 5 collections,
~1,100 files; write cases use a scratch collection of 400 × 5 KB files.

| scenario | p50 | p90 |
|---|---:|---:|
| prompt hook (pull `memory`, nothing new) | 327 ms | 396 ms |
| full pull, 5 collections (SessionStart) | 413 ms | 518 ms |
| push, nothing changed | 23 ms | 27 ms |
| push 1 file | 649 ms | 709 ms |
| pull 1 file | 395 ms | 527 ms |
| push 20 files | 879 ms | 1407 ms |
| pull 20 files | 580 ms | 1303 ms |
| first push, 400 files | 8.6 s | |
| first pull, 400 files | 5.3 s | |

A quiet prompt costs one round trip to R2; local work is ~3 ms. Pulling a small change is one
round trip too: new content rides inline in the manifest. Push hooks run async, so they
never delay Claude.

[CONTRIBUTING](CONTRIBUTING.md) · [SECURITY](SECURITY.md) · [MIT](LICENSE)
