# Slice

Slice is a command-line tool written in Rust that allows you to slice the contents of a file using syntax similar to Python's slice notation.

![test_workflow](https://github.com/ChanTsune/slice/actions/workflows/test.yml/badge.svg)
[![Crates.io][crates-badge]][crates-url]

[crates-badge]: https://img.shields.io/crates/v/slice-command.svg
[crates-url]: https://crates.io/crates/slice-command

## Installation

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

To use Slice, run the following command:

```
slice [options] <slice> <file...>
```

`<file>` is the name of the file you want to slice, and `<slice>` is the slice syntax you want to apply to the file.
If `<file>` is not specified, `slice` will read from standard input.

The slice syntax is similar to Python's slice syntax, with the format `start:end:step`.
Each value is optional and, if omitted, defaults to the beginning of the file, the end of the file, and a step of 1, respectively.

## Examples

Here are some examples of how to use Slice:

```
slice 10:20 file.txt
```

This command slices the contents of `file.txt` from line 10 to line 20.

```
slice :100:2 file.txt
```

This command slices the contents of `file.txt` from the beginning of the file to line 100, skipping every second line.

For more details, run:

```sh
slice --help
```

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
