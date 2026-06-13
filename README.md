# Slice

Slice is a command-line tool written in Rust that allows you to slice the contents of a file using syntax similar to Python's slice notation.

![test_workflow](https://github.com/ChanTsune/slice/actions/workflows/test.yml/badge.svg)
[![Crates.io][crates-badge]][crates-url]

[crates-badge]: https://img.shields.io/crates/v/slice-command.svg
[crates-url]: https://crates.io/crates/slice-command

## Installation

### Via Homebrew

```sh
brew install chantsune/tap/slice
```

### Via Nix

```sh
nix-env --install -f https://github.com/chantsune/slice/tarball/main
```

### Via Cargo

```sh
cargo install slice-command
```

### Prebuilt binaries

Prebuilt archives for Linux, macOS, and Windows (x86 and ARM) are published on
the [GitHub Releases](https://github.com/ChanTsune/slice/releases) page; see that
page for the targets covered by the latest release. Each archive bundles shell
completions (`complete/`) and the man page (`doc/`) alongside the binary.

[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) fetches and
installs the matching archive automatically:

```sh
cargo binstall slice-command
```

### From Source (via Cargo)

```sh
git clone https://github.com/ChanTsune/slice.git
cd slice
cargo install --path .
```

After building, add the binary to your PATH to use it globally.

## Usage

To use `slice`, run the following command:

```sh
slice [options] <slice> <file...>
```

`<file>` is the name of the file you want to slice, and `<slice>` is the slice syntax you want to apply to the file.
If `<file>` is not specified, `slice` will read from standard input.

The slice syntax is similar to Python's slice syntax, with the format `start:end:step`.
Each value is optional and, if omitted, defaults to the beginning of the file, the end of the file, and a step of 1, respectively.
Negative `start` and `end` values count back from the end of the input, exactly like Python: `-N` means `length - N`, and out-of-range values clamp to the input instead of erroring.

## Examples

Here are some examples of how to use `slice`:

```sh
slice 10:20 file.txt
```

This command slices the contents of `file.txt` from line 10 to line 20.

```sh
slice :100:2 file.txt
```

This command slices the contents of `file.txt` from the beginning of the file to line 100, skipping every second line.

```sh
slice 5:+10 file.txt
```

This command is the same as `slice` 5:15 file.txt`.

```sh
slice -5: file.txt
```

This command prints the last five lines of `file.txt`, like `tail -n 5`. The
usual `head`/`tail` invocations map onto tail-relative slices:

| Command              | Equivalent            |
| -------------------- | --------------------- |
| `slice -5: file`     | `tail -n 5 file`      |
| `slice -b -5: file`  | `tail -c 5 file`      |
| `slice :-5 file`     | `head -n -5 file` (GNU) |
| `slice -b :-5 file`  | `head -c -5 file` (GNU) |

The bounds follow Python rather than coreutils where the two disagree: `-0`
equals `0`, so `slice -0:` selects the whole input (where `tail -n 0` selects
nothing) and `slice :-0` selects nothing (where GNU `head -n -0` selects
everything).

A tail-relative `start` (`-N:`) cannot emit anything until the input ends, and
holds the last `N` elements in memory while it reads — the same shape as
`tail`. A tail-relative `end` (`:-N`) stays streaming: each element is emitted
as soon as `N` more have arrived, so only `N` elements are ever held. In byte
mode on regular files both forms resolve against the file size up front and
seek straight to the data, buffering nothing.

```sh
find . -type f -print0 | slice 0:100 -z
```

With `-z` (`--null`), records are split on NUL (`\0`) instead of newlines, so it
interoperates with `find -print0`, `xargs -0`, and `grep -z`.

```sh
slice 0:3 --delimiter '\t' -e data.tsv
```

By default `--delimiter` is taken literally. Add `-e` (`--escape`) to interpret
backslash escapes in the delimiter: `\t`, `\n`, `\r`, `\0`, `\\`, and `\xHH`
(an arbitrary byte, e.g. `\xff`). This command slices the first three
tab-separated fields.

For more details, run:

```sh
slice --help
```

## Shell completions and man page

`slice --generate <KIND>` prints a completion script or the man page to standard
output. `<KIND>` is one of `complete-bash`, `complete-zsh`, `complete-fish`,
`complete-powershell`, or `man`. Redirect the output to wherever your shell or
`man` looks for it, for example:

```sh
# bash (any bash-completion completions directory works)
mkdir -p ~/.local/share/bash-completion/completions
slice --generate complete-bash > ~/.local/share/bash-completion/completions/slice

# zsh (any directory on your $fpath works)
mkdir -p ~/.zfunc  # then add `fpath=(~/.zfunc $fpath)` to ~/.zshrc
slice --generate complete-zsh > ~/.zfunc/_slice

# fish
mkdir -p ~/.config/fish/completions
slice --generate complete-fish > ~/.config/fish/completions/slice.fish

# man page (~/.local/share/man/man1 or /usr/local/share/man/man1)
mkdir -p ~/.local/share/man/man1
slice --generate man > ~/.local/share/man/man1/slice.1
```

The prebuilt archives ship these files ready-made in `complete/` and `doc/`.

## Docker

```sh
docker build -t slice .
docker run -v `pwd`:`pwd` -w `pwd` --rm -i slice
```

## Development

### Dev Container

Open the repository in a [Dev Container](https://containers.dev/) (VS Code "Reopen in Container" or GitHub Codespaces) to get a ready-to-use environment.
The container installs the toolchain declared in `flake.nix` via Nix and activates it automatically with [direnv](https://direnv.net/), so `cargo test` works out of the box.

### Tests

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

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
