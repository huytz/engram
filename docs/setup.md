# Setup

## Install

**Claude Code (recommended):**

```sh
claude plugin marketplace add huytz/engram && claude plugin install engram@engram
```
In a new session, run `/engram:setup`: it guides the steps below. Until setup is done, every
session start shows `engram: not set up yet`.

**Codex:**

```sh
codex plugin marketplace add huytz/engram && codex plugin add engram@engram
```
Trust the 4 engram hooks when Codex asks, then ask it to run the engram `setup` skill. Codex has
no `autoMemoryDirectory`, so the session-start hook gives Codex the memory directory and
`MEMORY.md`. `apply_patch` edits are pushed the same way as Claude's Edit/Write.

The plugin brings the hooks, the `engram` and `setup` skills, and a launcher that downloads the
pinned release on first use, checks its sha256 and links it as `~/.engram/bin/engram`.

**Binary only:** download from [Releases](https://github.com/huytz/engram/releases) or run
`cargo install --git https://github.com/huytz/engram`. Then use `engram install-hooks` in place of
`--plugin` below.

## Setup

**1. Bucket and token** (R2 shown; any S3-compatible store works)

```sh
npx wrangler r2 bucket create engram
```
Then Cloudflare dashboard → R2 → *Manage API tokens* → **Object Read & Write**, limited to
that bucket. Keep the key ID, secret, and endpoint `https://<account-id>.r2.cloudflarestorage.com`.

**2. First device**

```sh
ENGRAM_ACCESS_KEY=<KEY_ID> ENGRAM_SECRET_KEY=<SECRET> ~/.engram/bin/engram init \
  --remote s3://engram --endpoint https://<account-id>.r2.cloudflarestorage.com --plugin
```
Every flag can come from the environment: `ENGRAM_REMOTE`, `ENGRAM_ENDPOINT`,
`ENGRAM_ACCESS_KEY`, `ENGRAM_SECRET_KEY`. `--plugin` also sets `autoMemoryDirectory`. Without the
plugin, run `engram install-hooks` after init.
Your existing auto-memory is imported and pushed. **Back up `~/.engram/identity.txt`**: it is
the only key to the remote copy.

**3. Every other device**

Copy `identity.txt` into `~/.engram/`, or set `ENGRAM_IDENTITY='AGE-SECRET-KEY-…'`, then run
the same command. Existing memory is merged.
Start a new Claude Code session.

## Commands

| | |
|---|---|
| `engram pull` / `push` / `sync` | fetch and merge, upload, both |
| `engram status` | devices, sequence numbers, pending changes |
| `engram collection add <name> <path> [--exclude sub]` | sync another directory (same name on every device) |
| `engram plugins` | which Claude Code plugins each device is missing |
| `engram install-hooks [--plugin]` / `uninstall-hooks` | add or remove the hooks in `~/.claude/settings.json` (`--plugin`: only `autoMemoryDirectory`) |
| `engram hook-timing on\|off` | show or hide per-hook timings in the agent UI (default off) |

Synced by default: `memory` and `skills` (`~/.claude/skills`, minus `synced/`). Add more with
`collection add`: `~/.claude/plans`, a project's `.claude/` (rules, skills; exclude
`settings.local.json`), any directory of notes. Plugins are compared, not copied. Never sync
session transcripts or caches.

## Cost

R2 free tier per month: 10 GB storage, 1 M Class A requests (PUT, LIST), 10 M Class B
(GET, HEAD); egress free. Beyond that: $0.015/GB-month, $4.50/M Class A, $0.36/M Class B
([pricing](https://developers.cloudflare.com/r2/pricing/)).

engram with two devices and five collections uses about 1 MB of storage plus one conditional
GET for `memory` per prompt (the device LIST is reused for 10 minutes), and two PUTs per saved
memory. At 200 prompts a day that is a few k Class A and ~30 k Class B per month, well inside
the free tier: **$0**.

## Limits

- Claude Code loads only the first 200 lines / 25 KB of `MEMORY.md`. engram merges the index
  but does not shorten it.
- One memory for all projects (`autoMemoryDirectory` is global).
- `ENGRAM_HOME` moves `~/.engram`; `file:///dir` is a valid remote for testing.
- Hook timings (`engram pull 412ms · …`) are hidden by default; `engram hook-timing on` shows them
  (`"hook_timing"` in `config.json`).
- The prompt hook reuses the device list for `"device_list_ttl"` seconds (default 600), so a new
  device shows up at the next session start or within 10 minutes; `0` LISTs on every prompt.
- `"background_pull": true` (default) makes the prompt hook start the pull in a detached process
  and return; its results (`~/.engram/inbox/`) reach Claude on the next prompt. `false` waits for it.
- TLS roots are read from the OS once a day into `~/.engram/roots.pem`; delete it after adding a CA.
