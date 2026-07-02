# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- next-header -->

## [Unreleased]

## [0.6.0] - 2026-07-02

### Added

- Negative `step` values, exactly like Python: `::-1` reverses the input, so
  `slice ::-1 file` works like `tac`. Works in every mode (lines, `-b`,
  `--delimiter`, `-z`); buffers the whole input in memory.
- `--translate` flag that prints the equivalent `head`/`tail`/`sed`/`awk`/`dd`
  command for a range, for porting a `slice` call to a box that lacks it.
- A cheatsheet at <https://chantsune.github.io/slice/> mapping common
  `head`/`tail`/`sed`/`awk`/`dd` recipes to `slice`.
- One-line install for Linux/macOS (`curl … | sh`) and Windows (`irm … | iex`).
- `--max-record-size <SIZE|unlimited>` as an opt-in guard for line/custom
  delimiter tail-relative and reverse ranges. The default remains unlimited
  for compatibility.
- Releases now include a `SHA256SUMS` manifest to verify downloads
  (`sha256sum -c SHA256SUMS`).
- Prebuilt binary for RISC-V Linux (`riscv64gc-unknown-linux-musl`); the
  `curl … | sh` installer now resolves RISC-V hosts.

## [0.5.0] - 2026-06-13

### Added

- Tail-relative (negative) `start` and `end` values, exactly like Python: `-N`
  counts back from the end of the input, so `slice -5: file` behaves like
  `tail -n 5 file` and `slice :-3` drops the last three elements. Out-of-range
  values clamp to the input instead of erroring.
- `--explain` flag that describes what a range selects (0-based indices,
  1-based positions, and element count) and exits without reading any input.
- `-b`/`--bytes` as the byte-mode flag. The old `-c` is kept as a hidden alias
  for backward compatibility.
- `-z`/`--null` to use NUL (`\0`) as the delimiter.
- `-e`/`--escape` to interpret backslash escapes (`\t`, `\n`, `\r`, `\0`,
  `\\`, `\xHH`) in `--delimiter`.
- `--generate <KIND>` flag that prints a shell completion script
  (`complete-bash`, `complete-zsh`, `complete-fish`, `complete-powershell`) or
  the man page (`man`) to stdout, without reading any input. Release archives
  now ship the generated completions (`complete/`) and man page (`doc/`).
- `cargo binstall slice-command` installs the prebuilt release binaries.
- Prebuilt release binaries for Windows on ARM (`aarch64-pc-windows-msvc`)
  and 64-bit ARM Linux (`aarch64-unknown-linux-musl`).

### Changed

- Substantially improved throughput in every mode: whole-input ranges (`:`)
  copy the input verbatim, unit-step ranges bulk-copy the selected window
  (seeking past the skipped prefix on regular files), delimiter scanning uses
  `memchr`/`memmem`, and stepped slices no longer allocate per chunk.
- Unreadable files now follow `head(1)` conventions: the error is reported on
  stderr, the remaining files are still processed, and the exit status
  reflects the failure.
- Range parse errors now name the failing field and suggest valid forms.

### Fixed

- A broken pipe (e.g. piping into `head`) is treated as a normal early stop
  instead of a failure.
- Ranges with more than three `:`-separated parts are rejected instead of
  silently ignoring the excess.
- A relative end (`+` or `+-`) without a count is rejected instead of
  selecting an unintended range.

## [0.4.3] - 2026-06-03

### Changed

- The minimum supported Rust version is now 1.85.

### Fixed

- Relative range arithmetic (`start:+N`, `start:+-N`) saturates instead of
  overflowing on huge values.

## [0.4.2] - 2024-11-13

### Changed

- The Docker image is now built `FROM scratch` instead of a distroless base.

## [0.4.1] - 2024-03-27

### Added

- Nix flake support.

## [0.4.0] - 2023-10-16

### Added

- `--delimiter` option to slice by an arbitrary delimiter instead of line
  breaks.
- Relative range ends (experimental): `start:+N` selects `N` elements starting
  at `start`, and `start:+-N` selects the window from `start - N` to
  `start + N`.

## [0.3.1] - 2023-08-22

### Changed

- Reduced memory allocations while slicing.

## [0.3.0] - 2023-06-26

### Added

- `--io-buffer-size` option (experimental) to tune the I/O buffer size,
  accepting data-unit suffixes such as `KB` and `MiB`.

### Changed

- stdout is locked while processing multiple files, improving throughput.

### Fixed

- `--io-buffer-size` rejects `0`.

## [0.2.2] - 2023-06-20

### Added

- aarch64 Docker image.

### Changed

- Improved throughput by locking stdin/stdout and flushing output explicitly.
- The Docker image is statically linked, based on `distroless/cc`, and uses
  `ENTRYPOINT` so arguments can be passed to the container directly.

## [0.2.1] - 2023-05-21

### Added

- Dockerfile for running slice as a container image.

### Fixed

- Slicing non-UTF-8 input no longer fails; streams are processed as raw bytes
  ([#24](https://github.com/ChanTsune/slice/issues/24)).

## [0.2.0] - 2023-05-08

### Changed

- `-l` and `-c` are mutually exclusive and can no longer be combined.

### Fixed

- No line break is appended to the output when the input does not end with
  one.

## [0.1.0] - 2023-04-22

### Added

- `-q` flag to suppress `==> file <==` headers when examining multiple files
  ([#5](https://github.com/ChanTsune/slice/pull/5)).

## [0.0.0] - 2023-04-20

### Added

- Initial release: slice lines (`-l`, default) or characters (`-c`) of files
  or stdin using Python-like `start:end:step` ranges, printing `==> file <==`
  headers when multiple files are given.

<!-- next-url -->
[Unreleased]: https://github.com/ChanTsune/slice/compare/0.6.0...HEAD
[0.6.0]: https://github.com/ChanTsune/slice/compare/0.5.0...0.6.0
[0.5.0]: https://github.com/ChanTsune/slice/compare/0.4.3...0.5.0
[0.4.3]: https://github.com/ChanTsune/slice/compare/0.4.2...0.4.3
[0.4.2]: https://github.com/ChanTsune/slice/compare/0.4.1...0.4.2
[0.4.1]: https://github.com/ChanTsune/slice/compare/0.4.0...0.4.1
[0.4.0]: https://github.com/ChanTsune/slice/compare/0.3.1...0.4.0
[0.3.1]: https://github.com/ChanTsune/slice/compare/0.3.0...0.3.1
[0.3.0]: https://github.com/ChanTsune/slice/compare/0.2.2...0.3.0
[0.2.2]: https://github.com/ChanTsune/slice/compare/0.2.1...0.2.2
[0.2.1]: https://github.com/ChanTsune/slice/compare/0.2.0...0.2.1
[0.2.0]: https://github.com/ChanTsune/slice/compare/0.1.0...0.2.0
[0.1.0]: https://github.com/ChanTsune/slice/compare/0.0.0...0.1.0
[0.0.0]: https://github.com/ChanTsune/slice/releases/tag/0.0.0
