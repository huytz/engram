# engram

**One Claude Code memory for all your devices.**

An *engram* is the trace a memory leaves in the brain. Claude Code builds its own as it works
with you: auto-memory (`MEMORY.md` + topic files) and the skills you write. They never leave the
machine they were formed on. engram syncs them through a bucket you own (Cloudflare R2, AWS S3,
MinIO), encrypted before upload, so every device shares one brain.

```
                                   🧠
   device 1                        R2                        device 2
┌──────────────┐   push ──▶   ┌─────────────┐   ◀── push   ┌──────────────┐
│ memory       │              │ memory/<d>  │              │ memory       │
│ skills       │   ◀── pull   │ skills/<d>  │   pull ──▶   │ skills       │
│ plans        │              │ plans/<d>   │              │ plans        │
│ rules        │              │ plugins/<d> │              │ rules        │
│ plugin list  │              │ <any>/<d>   │              │ plugin list  │
└──────────────┘              └─────────────┘              └──────────────┘
```

1. Claude saves a memory on device 1. A hook **pushes** it within seconds, encrypted with
   [age](https://age-encryption.org), into that device's own prefix.
2. Device 2 **pulls** at its next prompt (one small GET), merges `MEMORY.md`, and Claude sees
   the new entry in that same prompt.

Each device writes only its own prefix: no server, no locks, nothing overwritten. Conflicts
keep the newer file and save the other as `name.conflict-<device>.md`. Deletions propagate.
Unofficial; built only on documented Claude Code features (`autoMemoryDirectory`, hooks).

## Install

Download from [Releases](https://github.com/huytz/engram/releases) (macOS, Linux) or
`cargo install --git https://github.com/huytz/engram`.

## Setup

**1. Bucket and token** (R2 shown; any S3-compatible store works)

```sh
npx wrangler r2 bucket create engram
```
Then Cloudflare dashboard → R2 → *Manage API tokens* → **Object Read & Write**, limited to
that bucket. Keep the key ID, secret, and endpoint `https://<account-id>.r2.cloudflarestorage.com`.

**2. First device**

```sh
engram init --remote s3://engram --endpoint https://<account-id>.r2.cloudflarestorage.com \
  --access-key <KEY_ID> --secret-key <SECRET>
engram install-hooks
```
Your existing auto-memory is imported and pushed. **Back up `~/.engram/identity.txt`**: it is
the only key to the remote copy.

**3. Every other device**

Copy `identity.txt` into `~/.engram/`, run the same two commands. Existing memory is merged.
Start a new Claude Code session.

## Commands

| | |
|---|---|
| `engram pull` / `push` / `sync` | fetch and merge, upload, both |
| `engram status` | devices, sequence numbers, pending changes |
| `engram collection add <name> <path> [--exclude sub]` | sync another directory (same name on every device) |
| `engram plugins` | which Claude Code plugins each device is missing |
| `engram install-hooks` / `uninstall-hooks` | add or remove the hooks in `~/.claude/settings.json` |

Synced by default: `memory` and `skills` (`~/.claude/skills`, minus `synced/`). Add more with
`collection add`: `~/.claude/plans`, a project's `.claude/` (rules, skills; exclude
`settings.local.json`), any directory of notes. Plugins are compared, not copied. Never sync
session transcripts or caches.

## Cost

R2 free tier per month: 10 GB storage, 1 M Class A requests (PUT, LIST), 10 M Class B
(GET, HEAD); egress free. Beyond that: $0.015/GB-month, $4.50/M Class A, $0.36/M Class B
([pricing](https://developers.cloudflare.com/r2/pricing/)).

engram with two devices and five collections uses about 1 MB of storage plus one LIST and one
conditional GET per collection per prompt, and two PUTs per saved memory. At 200 prompts a day
that is ~30 k Class A and ~30 k Class B per month, well inside the free tier: **$0**.

## Limits

- Claude Code loads only the first 200 lines / 25 KB of `MEMORY.md`. engram merges the index
  but does not shorten it.
- One memory for all projects (`autoMemoryDirectory` is global).
- `ENGRAM_HOME` moves `~/.engram`; `file:///dir` is a valid remote for testing.

[CONTRIBUTING](CONTRIBUTING.md) · [SECURITY](SECURITY.md) · [MIT](LICENSE)
