<p align="center">
  <img src="docs/logo.svg" alt="slice" width="210">
</p>

# Slice

[![CI][ci-badge]][ci-url]
[![Crates.io][crates-badge]][crates-url]
[![Downloads][downloads-badge]][crates-url]
[![License][license-badge]](#license)
[![MSRV][msrv-badge]][crates-url]

Slice is a command-line tool written in Rust that allows you to slice the contents of a file using syntax similar to Python's slice notation.

[ci-badge]: https://github.com/ChanTsune/slice/actions/workflows/test.yml/badge.svg
[ci-url]: https://github.com/ChanTsune/slice/actions/workflows/test.yml
[crates-badge]: https://img.shields.io/crates/v/slice-command.svg
[crates-url]: https://crates.io/crates/slice-command
[downloads-badge]: https://img.shields.io/crates/d/slice-command.svg
[license-badge]: https://img.shields.io/badge/license-Apache--2.0_OR_MIT-blue.svg
[msrv-badge]: https://img.shields.io/crates/msrv/slice-command.svg

## Installation

### Via install script

Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/ChanTsune/slice/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/ChanTsune/slice/main/install.ps1 | iex
```

The script downloads the matching prebuilt binary from the latest GitHub release
and installs it. Set `SLICE_VERSION` to pin a version or `SLICE_INSTALL_DIR` to
choose the install location.

### Prebuilt binaries

Prebuilt archives for Linux (x86, ARM, and RISC-V), macOS, Windows (x86 and
ARM), and FreeBSD are published on the
[GitHub Releases](https://github.com/ChanTsune/slice/releases) page; see that
page for the targets covered by the latest release. Each archive bundles shell
completions (`complete/`) and the man page (`doc/`) alongside the binary.

[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall) fetches and
installs the matching archive automatically:

```sh
cargo binstall slice-command
```

### Via Homebrew

```sh
brew install chantsune/tap/slice
```

### Via Nix

Try without installing:

```sh
nix run github:ChanTsune/slice -- :5 file.txt
```

Install permanently:

```sh
nix profile add github:ChanTsune/slice
```

To contribute, enter the development shell after cloning:

```sh
nix develop
# or automatically with direnv:
direnv allow
```

Non-flake fallback:

```sh
nix-env --install -f https://github.com/ChanTsune/slice/tarball/main
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
Negative `start` and `end` values count back from the end of the input, exactly like Python: `-N` means `length - N`, and out-of-range values clamp to the input instead of erroring.
A negative `step` selects in reverse — `slice ::-1 file.txt` reverses the file like `tac` — and buffers the whole input in memory.

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

This command is the same as `slice 5:15 file.txt`.

```sh
slice -5: file.txt
```

This command prints the last five lines of `file.txt`, like `tail -n 5`. The
usual `head`/`tail`/`sed`/`awk`/`dd` line and byte ranges all map onto one slice
syntax:

<!-- CHEATSHEET:START -->
The recipes below are generated from `docs/cheatsheet.toml`; the full
version (byte ranges, every-Nth-line, NUL records, caveats, and a
"when NOT to use slice" section) lives at
<https://chantsune.github.io/slice/>.

#### Print a range of lines (head, tail, sed, awk)

| Task | coreutils / sed / awk / dd | slice |
| --- | --- | --- |
| First 5 lines | `head -n 5` | `slice :5` |
| Last 5 lines | `tail -n 5` | `slice -5:` |
| All but the last 5 lines | `head -n -5` | `slice :-5` |
| All but the last line | `sed '$d'  /  head -n -1` | `slice :-1` |
| All but the first line | `sed '1d'  /  tail -n +2` | `slice 1:` |
| From line N to the end | `tail -n +3` | `slice 2:` |
| Lines 2 through 5 | `sed -n '2,5p'  /  awk 'NR>=2&&NR<=5'` | `slice 1:5` |
| Line 7 only | `sed -n '7p'  /  awk 'NR==7'` | `slice 6:7` |
| From line 10 to the end | `sed -n '10,$p'` | `slice 9:` |

<!-- CHEATSHEET:END -->

The bounds follow Python rather than coreutils where the two disagree: `-0`
equals `0`, so `slice -0:` selects the whole input (where `tail -n 0` selects
nothing) and `slice :-0` selects nothing (where GNU `head -n -0` selects
everything).

A tail-relative `start` (`-N:`) cannot emit anything until the input ends — the
same shape as `tail` — whereas a tail-relative `end` (`:-N`) streams its output
as it reads.

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

```sh
slice --chars 0:5 file.txt
```

With `--chars`, elements are UTF-8 characters (code points), exactly like
Python `str` slicing — `slice --chars ::-1` reverses text the way `s[::-1]`
does. Bytes that are not valid UTF-8 pass through unchanged, counted as one
character each, so arbitrary data never fails. As in Python, an emoji composed
of multiple code points counts as several characters, not one visible glyph.

```sh
slice --graphemes 0:5 file.txt
```

`--graphemes` slices by user-perceived character (Unicode extended grapheme
cluster) instead: 👨‍👩‍👧 is five elements under `--chars` but one here, and
`\r\n` counts as one. Invalid bytes pass through the same way, one element
each.

For more details, run:

```sh
slice --help
```

## Translate to a portable command

`slice --translate` prints the nearest equivalent `head`/`tail`/`sed`/`awk`/`dd`
command for a range, then exits without reading input — the answer to "I used
`slice` here, but the box that runs this script doesn't have it":

```sh
$ slice --translate=posix 1:5
# posix
sed -n '2,5p'

$ slice -b --translate=posix 5:15
# posix
dd bs=1 skip=5 count=10 2>/dev/null
```

Pass an explicit dialect (`posix`/`bsd`/`gnu`/`awk`/`all`) or omit it for the
platform's native toolset. Each command is labelled by dialect
(`# posix`/`# bsd`/`# gnu`/`# awk`); portability caveats appear inline as
parenthetical notes; and ranges with no faithful single-command equivalent say
so instead of misleading you.

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

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development environment (Dev
Container) and how to run and regenerate the tests.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
