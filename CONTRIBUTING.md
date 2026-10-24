# Contributing

Thanks for taking a look. engram is small on purpose; please keep changes focused.

## Development

```sh
cargo build
cargo test                       # unit tests
cargo build --release && scripts/e2e.sh   # two simulated devices over a file:// remote
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

`ENGRAM_HOME` points engram at a different state directory, so you can test without touching
your real `~/.engram`. A `file:///abs/dir` remote needs no credentials. To exercise the S3 code
path locally, run [MinIO](https://min.io) and pass `--remote s3://bucket --endpoint http://127.0.0.1:9000`.

## Pull requests

- One change per PR, with a short description of the behaviour before and after.
- Add or extend a step in `scripts/e2e.sh` when you change sync or merge behaviour.
- CI must be green (fmt, clippy with `-D warnings`, tests, e2e on Linux and macOS).
- Keep comments to one line; put longer reasoning in the PR description.

## Reporting bugs

Open an issue with: engram version (`engram --version`), OS, the command you ran, the relevant
lines from `~/.engram/engram.log`, and whether the remote is R2, S3 or `file://`. Never paste
`identity.txt`, access keys, or the contents of your memory files.
