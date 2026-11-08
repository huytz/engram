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

## Reporting a vulnerability

Please do not open a public issue. Email the maintainer (see the GitHub profile of
[@huytz](https://github.com/huytz)) with a description and reproduction steps. You should get a
reply within a week.
