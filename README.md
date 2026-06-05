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
Multiple ranges can be provided as a comma-separated list:

```sh
slice 0:5,10:15 file.txt
```

Ranges are evaluated in the order written and are streamed without buffering the whole input. Because streams cannot be rewound, overlapping or backward ranges such as `10:15,0:5` or `0:5,3:7` are rejected.

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
slice 0:5,10:15 file.txt
```

This command writes lines 0 through 4, then lines 10 through 14.

```sh
slice 5:+10 file.txt
```

This command is the same as `slice` 5:15 file.txt`.

For more details, run:

```sh
slice --help
```

## Docker

```sh
docker build -t slice .
docker run -v `pwd`:`pwd` -w `pwd` --rm -i slice
```

## Development

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
