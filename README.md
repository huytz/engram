# engram

**One Claude Code memory for all your devices.**

An *engram* is the trace a memory leaves in the brain. Claude Code forms its own: the
auto-memory directory (`MEMORY.md` + topic files) and the skills you write. They stay on the
machine where they were formed. engram carries them between your devices through a bucket you
own (Cloudflare R2, AWS S3, MinIO), encrypted before upload.

```
   laptop                           your bucket                          desktop
┌────────────────┐   push    ┌──────────────────────┐   pull    ┌────────────────┐
│ Claude writes  │ ────────▶ │ memory/<laptop>/     │ ────────▶ │ next prompt:   │
│ a memory       │  (hook)   │   manifest + blobs 🔒 │  (hook,   │ Claude has it  │
└────────────────┘           │ memory/<desktop>/ …  │   1 GET)  └────────────────┘
                             └──────────────────────┘
```

1. Claude saves a memory. A hook pushes it: files encrypted with [age](https://age-encryption.org),
   stored as content-addressed blobs under this device's own prefix, then a manifest.
2. The other device pulls at its next prompt (one conditional GET), merges `MEMORY.md` line by
   line, and the new index lines are shown to Claude in that same prompt.

Each device writes only its own prefix, so there is no server, no lock and no lost update.
Conflicts: newest wins, loser kept as `name.conflict-<device>.md`. Deletions use tombstones.
Unofficial; uses only documented Claude Code features (`autoMemoryDirectory`, hooks).

## Install

[Releases](https://github.com/huytz/engram/releases) (macOS/Linux, arm64/x86_64) or
`cargo install --git https://github.com/huytz/engram` (Rust 1.85+).

## Setup

1. Bucket + token. R2: `npx wrangler r2 bucket create engram`, then dashboard → R2 → API
   tokens → *Object Read & Write* on that bucket. Endpoint: `https://<account-id>.r2.cloudflarestorage.com`.
2. First device:
   ```sh
   engram init --remote s3://engram --endpoint https://<account-id>.r2.cloudflarestorage.com \
     --access-key <KEY_ID> --secret-key <SECRET>
   engram install-hooks
   ```
   Device name = `hostname -s`. Existing auto-memory is imported into `~/.engram/memory`.
   **Back up `~/.engram/identity.txt`**; it is the only key to the remote copy.
3. Other devices: copy `identity.txt` to `~/.engram/`, run the same two commands. Existing
   memory is merged. Open a new Claude Code session afterwards.

## Commands

| | |
|---|---|
| `engram pull` / `push` / `sync` | fetch+merge, upload, both |
| `engram status` | devices, seqs, pending changes |
| `engram install-hooks` / `uninstall-hooks` | edit `~/.claude/settings.json` |
| `engram collection add <name> <path> [--exclude sub]` | sync another directory (same name on every device) |
| `engram plugins` | diff installed Claude Code plugins across devices |

Default collections: `memory`, `skills` (`~/.claude/skills` minus `synced/`). Worth adding:
`~/.claude/plans`, a project's `.claude/` (exclude `settings.local.json`). Do not sync
session transcripts or caches.

## Limits

- Claude Code loads only the first 200 lines / 25 KB of `MEMORY.md`. engram merges, it does not
  shorten. Keep the index small.
- `autoMemoryDirectory` is global: one memory for all projects.
- `ENGRAM_HOME` overrides `~/.engram`; `file:///dir` works as a remote for tests.

[CONTRIBUTING](CONTRIBUTING.md) · [SECURITY](SECURITY.md) · [MIT](LICENSE)
