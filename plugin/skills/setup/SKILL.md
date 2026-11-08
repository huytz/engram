---
name: setup
description: Set up engram memory sync on this device after installing the plugin — create the Cloudflare R2 bucket and token (or use any S3 store), run engram init, join an existing setup from another device. Use when the session says "engram: not set up yet", or the user runs /engram:setup, asks to set up / configure / connect engram, or adds a second device.
---

# engram setup

Goal: `~/.engram/config.json` exists, `engram status` lists this device, and the next session
syncs. The binary is `~/.engram/bin/engram` (the plugin downloads it at session start; in Claude
Code's Bash tool plain `engram` works too).

**Secrets never go through the chat.** Don't ask the user to paste keys or the identity to you, and
don't put them in a command you run. The user runs the `init` command themselves: in their own
terminal, or in Claude Code as `! <command>` only when the secrets come from a secret manager
(`$(op read …)`), so that only the reference shows up in the transcript.

## 1. Already set up?

Run `~/.engram/bin/engram status`. If it lists devices, setup is done; skip to step 5. If the
binary is missing, ask the user to start a new session (the hook downloads it) and stop.

## 2. First device or another one?

Ask the user: "Is engram already running on another device with a bucket?"

- **Another device exists:** reuse that bucket, the token and the **same identity**
  (`~/.engram/identity.txt` on the old device, starts with `AGE-SECRET-KEY-`). A new identity
  can't read the remote. Go to step 4.
- **First device:** step 3.

## 3. Bucket and token (first device only)

Cloudflare R2 (free tier is enough). Other S3 stores work: use their endpoint.

1. Create the bucket: `npx wrangler r2 bucket create engram`, or use the dashboard → R2 → Create bucket.
2. Dashboard → R2 → **Manage API tokens** → Create token → permission **Object Read & Write**,
   limited to that bucket.
3. Keep the Access Key ID, the Secret Access Key and the endpoint
   `https://<account-id>.r2.cloudflarestorage.com`.

## 4. Init (the user runs this)

Give the user this command with the bucket and endpoint filled in. Leave the secrets as
placeholders:

```sh
ENGRAM_ACCESS_KEY='<key id>' ENGRAM_SECRET_KEY='<secret>' \
  ~/.engram/bin/engram init --remote s3://<bucket> \
  --endpoint https://<account-id>.r2.cloudflarestorage.com --plugin
```

- Another device: add `ENGRAM_IDENTITY='<AGE-SECRET-KEY-… line>'` in front, or copy
  `identity.txt` into `~/.engram/` before running it.
- With 1Password, the user can run it straight in Claude Code with
  `! ENGRAM_SECRET_KEY="$(op read 'op://<vault>/<item>/secret')" …`.
- `--plugin` sets Claude Code's `autoMemoryDirectory` and removes old engram hooks. Your existing
  Claude auto-memory is imported and pushed.
- Codex only (no Claude Code): `--plugin` is harmless, it only edits `~/.claude/settings.json`.

If init fails with `wrong identity`, the identity doesn't match the remote. Ask the user to
remove `~/.engram/config.json` and `~/.engram/identity.txt`, then rerun with the right identity.

## 5. Check and finish

1. Run `~/.engram/bin/engram status`: this device should be listed with a sequence number.
2. First device: tell the user to **back up `~/.engram/identity.txt`** (for example in a password
   manager). It is the only key to the remote copy.
3. Tell the user to start a new session: hooks and `autoMemoryDirectory` load at session start.
   In Codex, trust the 4 engram hooks when it asks ("Hooks need review").
4. Optional: `engram hook-timing on` shows the cost of each sync.
