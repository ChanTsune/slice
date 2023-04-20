# Slice

Slice is a command-line tool written in Rust that allows you to slice the contents of a file using syntax similar to Python's slice notation.

## Installation

To install `slice`, clone the GitHub repository and build it from source using the Rust package manager, Cargo.

```sh
git clone https://github.com/ChanTsune/slice-command.git
cd slice-command
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

Slice is licensed under the MIT License. See [LICENSE](LICENSE) for more information.
