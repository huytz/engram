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
  `~/.engram/engram.log` and the command exits 0.

## Out of scope

- Protecting memory files from other users or processes on the same machine. They are plain
  text, as Claude Code itself stores them.
- Hiding traffic patterns (when you sync, how many objects changed).

## Reporting a vulnerability

Please do not open a public issue. Email the maintainer (see the GitHub profile of
[@huytz](https://github.com/huytz)) with a description and reproduction steps. You should get a
reply within a week.
