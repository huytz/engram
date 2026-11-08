# Setup

## Install

**Plugin (Claude Code and Codex).** The plugin brings the hooks, an `engram` skill and a
launcher that downloads the matching release binary on first use (sha256-checked, into
`~/.engram/bin/`).

```sh
claude plugin marketplace add huytz/engram && claude plugin install engram@engram
codex plugin marketplace add huytz/engram && codex plugin add engram@engram
```
Then use `engram install-hooks --plugin` in step 2 below: it only sets `autoMemoryDirectory`
(a plugin cannot) and removes hooks a binary install added earlier, so they don't run twice.

**Binary only.** Download from [Releases](https://github.com/huytz/engram/releases) (macOS,
Linux) or `cargo install --git https://github.com/huytz/engram`, then `engram install-hooks`.

**Codex.** Codex has no `autoMemoryDirectory`, so at session start the hook gives Codex the
memory directory and `MEMORY.md`; the skill tells it how to save. Edits through `apply_patch`
are pushed like Claude's Edit/Write.

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
engram install-hooks            # or --plugin with the plugin
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
