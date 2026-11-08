# Security

## Model

- Everything engram uploads is encrypted on the device with [age](https://age-encryption.org)
  (X25519) before it reaches the bucket. The bucket operator sees object names
  (`<collection>/<device>/blobs/<sha256>`), sizes and timestamps, not content or file names.
- The age identity in `~/.engram/identity.txt` is the only key. It is shared between your
  devices and never uploaded. Lose it and the remote copy is unreadable; local files stay
  readable. Back it up in a password manager.
- `config.json` holds the bucket credentials in plain text with mode 0600. Scope the token to
  one bucket with object read/write only; engram never needs bucket-level or account rights.
- engram runs as Claude Code hooks. Hooks never fail the Claude turn: errors go to
  `~/.engram/engram.log`, show as a one-line warning, and the command exits 0.
- Remote data is checked before it touches disk: manifest keys must be plain relative paths
  inside the synced directory (no `..`, absolute paths, symlinked parents or case aliases),
  device names must match `[A-Za-z0-9._-]{1,64}`, every blob must hash to its name, and a
  manifest must name its own device and collection and carry a higher sequence number than the
  one last accepted. A device whose data fails these checks is skipped, not the whole pull.

## Integrity limits

- age gives confidentiality, not sender authentication: anyone with the identity's **public**
  key and write access to the bucket can produce a manifest that decrypts. Treat the public key
  (printed by `engram init`/`status`) as private, and the bucket token as a secret.
- Any device holding the identity can change or delete synced files on every other device;
  that is the point of sync. Retire a device with `engram device remove <name>` and rotate the
  bucket token.
- A bucket writer without the identity can still withhold or delete objects (stop sync), but
  can no longer replay an older manifest, move one to another prefix, or swap blob contents.
- Blob names are the plain SHA-256 of the content, so someone who can list the bucket can test
  whether it holds a file they already know, and see identical files across devices.


## Out of scope

- Protecting memory files from other users or processes on the same machine. They are plain
  text, as Claude Code itself stores them.
- Hiding traffic patterns (when you sync, how many objects changed).

## Signs of a misbehaving device

engram prints a one-line warning (in the agent UI even with timings off, on stderr in the CLI,
and in `~/.engram/engram.log`) when:

- a device's data is skipped (bad manifest, replayed or copied manifest, wrong identity);
- a manifest contained unsafe file keys, which were ignored;
- a device name appears for the first time;
- this device's own manifest on the remote is missing or was overwritten by someone else
  (engram republishes it).

Also check `engram status` for devices you don't own, and `engram.log` for `from <device>` lines
you can't explain.

## If a device or the bucket token is compromised

Rotate the token **and** the identity: a new token alone leaves the old device able to read
everything; a new identity alone leaves it able to write junk to the bucket.

1. On a trusted device, run `engram device remove <name>`, then run it on every other device.
   This deletes the device's data and adds it to `ignored_devices`.
2. Create a new bucket token, revoke the old one, and update `config.json` on each device.
3. Make a new identity: on one device, move `~/.engram` aside, then run `engram init` against a
   **new, empty bucket**, which imports the current memory. Set up the other devices with
   `ENGRAM_IDENTITY` from that device, and delete the old bucket.
4. Review what the device changed: `grep 'from <name>' ~/.engram/engram.log`. Look at
   `~/.claude/skills/` and `~/.claude/settings.json`, and at shell rc files changed since the
   compromise.
5. Check: `engram pull` shows no warnings on every device, and `engram status` lists only your
   devices.

## Reporting a vulnerability

Please do not open a public issue. Email the maintainer (see the GitHub profile of
[@huytz](https://github.com/huytz)) with a description and reproduction steps. You should get a
reply within a week.
