# Contributing

Thanks for taking an interest in slice! Contributions of every kind are
welcome — bug reports, fixes, new features, documentation, tests, or a question
that nudges the docs to be clearer. No contribution is too small.

Reaching for AI tools to draft or review your changes is fine; there's no
judgment here. What matters is that whoever opens the pull request owns it:
understand what you're submitting, respond to review, and see it through to the
end. Please don't open a PR you aren't prepared to carry across the finish line.

## Development environment

Open the repository in a [Dev Container](https://containers.dev/) (VS Code
"Reopen in Container" or GitHub Codespaces) to get a ready-to-use environment.
The container installs the toolchain declared in `flake.nix` via Nix and
activates it automatically with [direnv](https://direnv.net/), so `cargo test`
works out of the box.

## Tests

Run the unit tests and the end-to-end CLI tests together:

```sh
cargo test
```

CLI behavior is locked under `tests/cmd/` via [`trycmd`]: each `*.toml` case runs the built
`slice` binary and compares its stdout, stderr, and exit code against sibling golden files.

After an intentional behavior change, regenerate the expected outputs and review the diff:

```sh
TRYCMD=overwrite cargo test --test cli   # update existing golden files in place
```

When adding a brand-new case, write its `*.toml` (plus `*.stdin` and/or `*.in/`), capture the
actual output with `TRYCMD=dump cargo test --test cli`, and copy `dump/<name>.stdout` /
`dump/<name>.stderr` into `tests/cmd/`. Keep OS-specific lines redacted with `[..]` (currently the
I/O-error message and the `--version` string).

[`trycmd`]: https://docs.rs/trycmd
