use crate::{
    cli,
    ext::{BufReadExt, IteratorExt},
    range::SliceRange,
};
use std::{
    fs,
    io::{self, stdin, stdout, BufRead, Read, Write},
    path::{Path, PathBuf},
};

#[inline]
fn buf_reader<R: Read>(reader: R, capacity: Option<usize>) -> io::BufReader<R> {
    if let Some(capacity) = capacity {
        io::BufReader::with_capacity(capacity, reader)
    } else {
        io::BufReader::new(reader)
    }
}

#[inline]
fn buf_writer<W: Write>(writer: W, capacity: Option<usize>) -> io::BufWriter<W> {
    if let Some(capacity) = capacity {
        io::BufWriter::with_capacity(capacity, writer)
    } else {
        io::BufWriter::new(writer)
    }
}

/// Copy the line slice selected by `range` from `input` to `output`.
#[inline]
pub fn line_mode<R: BufRead, W: Write>(
    input: R,
    mut output: W,
    range: &SliceRange,
) -> io::Result<()> {
    for line in input
        .lines_with_eol()
        .slice(range.start, range.end, range.step)
    {
        output.write_all(&line?)?;
    }
    output.flush()
}

/// Copy the delimiter-separated parts selected by `range` from `input` to
/// `output`. Each part keeps its trailing delimiter.
#[inline]
pub fn delimit_mode<R: BufRead, W: Write>(
    input: R,
    mut output: W,
    delimiter: &[u8],
    range: &SliceRange,
) -> io::Result<()> {
    for part in input
        .delimit_by(delimiter)
        .slice(range.start, range.end, range.step)
    {
        output.write_all(&part?)?;
    }
    output.flush()
}

/// Copy the byte slice selected by `range` from `input` to `output`.
#[inline]
pub fn byte_mode<R: BufRead, W: Write>(
    input: R,
    mut output: W,
    range: &SliceRange,
) -> io::Result<()> {
    for byte in input.bytes().slice(range.start, range.end, range.step) {
        output.write_all(&[byte?])?;
    }
    output.flush()
}

fn report_error(path: &Path, err: &io::Error) {
    eprintln!("slice: {}: {}", path.display(), err);
}

// A broken pipe means a downstream consumer (e.g. `head`) closed early; that is
// a normal stop request, not a failure, so we abort quietly without touching
// the exit status.
#[inline]
fn is_broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

#[inline]
fn multi<
    W: Write,
    R: BufRead,
    IW: Fn(fs::File) -> R,
    F: Fn(R, &mut W, &SliceRange) -> io::Result<()>,
>(
    targets: &[PathBuf],
    mut out: W,
    input_wrapper: IW,
    range: &SliceRange,
    print_header: bool,
    f: F,
) -> bool {
    let mut ok = true;
    for target in targets {
        // Open before printing the header so an unopenable file gets an error
        // on stderr instead of a header, and the remaining files are still
        // processed (same convention as head(1)/tail(1)).
        let file = match fs::File::open(target) {
            Ok(file) => file,
            Err(err) => {
                report_error(target, &err);
                ok = false;
                continue;
            }
        };
        let result = (|| {
            if print_header {
                writeln!(out, "==> {} <==", target.display())?;
            }
            f(input_wrapper(file), &mut out, range)
        })();
        if let Err(err) = result {
            if is_broken_pipe(&err) {
                return ok;
            }
            report_error(target, &err);
            ok = false;
        }
    }
    ok
}

/// Run the command described by `args` against stdin or the listed files.
/// Returns `Ok(true)` on success and `Ok(false)` when a runtime failure was
/// already reported on stderr. `Err` carries an invalid-usage message (e.g. a
/// bad `--delimiter` escape) for the caller to render; this function never
/// exits the process, so it is safe to drive from tests and fuzz harnesses.
pub fn entry(args: cli::Args) -> Result<bool, String> {
    let io_buffer_size = args.io_buffer_size();
    let delimiter = args.delimiter()?;
    if args.explain {
        let unit = if args.bytes {
            "byte"
        } else if delimiter.is_some() {
            "part"
        } else {
            "line"
        };
        print!("{}", args.range.explain(unit));
        return Ok(true);
    }
    if args.files.is_empty() {
        let input = buf_reader(stdin().lock(), io_buffer_size);
        let output = buf_writer(stdout().lock(), io_buffer_size);
        let result = if args.bytes {
            byte_mode(input, output, &args.range)
        } else if let Some(delimiter) = delimiter.as_deref() {
            delimit_mode(input, output, delimiter, &args.range)
        } else {
            line_mode(input, output, &args.range)
        };
        if let Err(err) = result {
            if is_broken_pipe(&err) {
                return Ok(true);
            }
            eprintln!("slice: {err}");
            return Ok(false);
        }
        Ok(true)
    } else {
        // A single file never gets a header, so -q only matters for 2+ files.
        let print_header = args.files.len() > 1 && !args.quiet_headers;
        let output = buf_writer(stdout().lock(), io_buffer_size);
        let ok = if args.bytes {
            multi(
                &args.files,
                output,
                |input| buf_reader(input, io_buffer_size),
                &args.range,
                print_header,
                |input, output, range| byte_mode(input, output, range),
            )
        } else if let Some(delimiter) = delimiter.as_deref() {
            multi(
                &args.files,
                output,
                |input| buf_reader(input, io_buffer_size),
                &args.range,
                print_header,
                |input, output, range| delimit_mode(input, output, delimiter, range),
            )
        } else {
            multi(
                &args.files,
                output,
                |input| buf_reader(input, io_buffer_size),
                &args.range,
                print_header,
                |input, output, range| line_mode(input, output, range),
            )
        };
        Ok(ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    mod line {
        use super::*;

        #[test]
        fn empty() {
            let mut out = Vec::new();
            line_mode(
                b"".as_slice(),
                &mut out,
                &SliceRange::from_str("::").unwrap(),
            )
            .expect("");

            assert_eq!(out, b"");
        }

        mod one_line {
            use super::*;

            #[test]
            fn no_slice() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\n".as_slice(),
                    &mut out,
                    &SliceRange::from_str("::").unwrap(),
                )
                .expect("");

                assert_eq!(out, b"slice command is simple string slicing command.\n");
            }

            #[test]
            fn skip_first() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\n".as_slice(),
                    &mut out,
                    &SliceRange::from_str("1:").unwrap(),
                )
                .expect("");

                assert_eq!(out, b"");
            }

            #[test]
            fn skip_over_input() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\n".as_slice(),
                    &mut out,
                    &SliceRange::from_str("2:").unwrap(),
                )
                .expect("");

                assert_eq!(out, b"");
            }

            #[test]
            fn drop_tail() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\n".as_slice(),
                    &mut out,
                    &SliceRange::from_str(":0").unwrap(),
                )
                .expect("");

                assert_eq!(out, b"");
            }

            #[test]
            fn step_two_slice() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\n".as_slice(),
                    &mut out,
                    &SliceRange::from_str("::2").unwrap(),
                )
                .expect("");

                assert_eq!(out, b"slice command is simple string slicing command.\n");
            }
        }

        mod multi_line {
            use super::*;

            #[test]
            fn no_slice() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                        .as_slice(),
                    &mut out,
                    &SliceRange::from_str("::").unwrap(),
                )
                    .expect("");

                assert_eq!(
                    out,
                    b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                );
            }

            #[test]
            fn skip_first() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                        .as_slice(),
                    &mut out,
                    &SliceRange::from_str("1:").unwrap(),
                )
                    .expect("");

                assert_eq!(out, b"Like a python slice syntax.\n");
            }

            #[test]
            fn drop_last() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                        .as_slice(),
                    &mut out,
                    &SliceRange::from_str(":1").unwrap(),
                )
                    .expect("");

                assert_eq!(out, b"slice command is simple string slicing command.\n");
            }

            #[test]
            fn step_two_slice() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\nLike a python slice syntax.\n".repeat(5)
                        .as_slice(),
                    &mut out,
                    &SliceRange::from_str("::2").unwrap(),
                )
                    .expect("");

                assert_eq!(
                    out,
                    b"slice command is simple string slicing command.\n".repeat(5)
                );
            }

            #[test]
            fn without_linebreak() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\nLike a python slice syntax."
                        .as_slice(),
                    &mut out,
                    &SliceRange::from_str("::").unwrap(),
                )
                .expect("");

                assert_eq!(
                    out,
                    b"slice command is simple string slicing command.\nLike a python slice syntax."
                );
            }

            #[test]
            fn binary_stream() {
                let mut out = Vec::new();
                line_mode(
                    b"slice\xaabinary stream\nslice binary\xaastream".as_slice(),
                    &mut out,
                    &SliceRange::from_str("::").unwrap(),
                )
                .expect("");

                assert_eq!(out, b"slice\xaabinary stream\nslice binary\xaastream");
            }
        }
    }

    mod broken_pipe {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A writer that fails every write with BrokenPipe, mimicking a
        // downstream consumer (e.g. `head`) that closed the pipe early.
        struct BrokenPipeWriter;

        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }

        static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

        fn temp_file(contents: &[u8]) -> PathBuf {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "slice-broken-pipe-{}-{}.txt",
                std::process::id(),
                id
            ));
            fs::write(&path, contents).expect("write temp file");
            path
        }

        #[test]
        fn aborts_quietly_and_reports_success() {
            let file = temp_file(b"line one\nline two\n");
            let ok = multi(
                std::slice::from_ref(&file),
                BrokenPipeWriter,
                io::BufReader::new,
                &SliceRange::from_str("::").unwrap(),
                false,
                |input, output, range| line_mode(input, output, range),
            );
            fs::remove_file(&file).ok();

            assert!(ok, "broken pipe must not fail the exit status");
        }

        #[test]
        fn preserves_earlier_failure() {
            let missing = std::env::temp_dir().join(format!(
                "slice-broken-pipe-missing-{}.txt",
                std::process::id()
            ));
            fs::remove_file(&missing).ok();
            let readable = temp_file(b"line one\nline two\n");

            // The missing file fails to open (a real error reported to stderr),
            // then the broken pipe aborts the rest; the earlier failure must
            // still be reflected in the returned status.
            let ok = multi(
                &[missing, readable.clone()],
                BrokenPipeWriter,
                io::BufReader::new,
                &SliceRange::from_str("::").unwrap(),
                false,
                |input, output, range| line_mode(input, output, range),
            );
            fs::remove_file(&readable).ok();

            assert!(!ok, "a failure before the broken pipe must be preserved");
        }
    }

    mod byte {
        use super::*;

        #[test]
        fn empty() {
            let mut out = Vec::new();
            byte_mode(
                b"".as_slice(),
                &mut out,
                &SliceRange::from_str("::").unwrap(),
            )
            .expect("");

            assert_eq!(out, b"");
        }

        #[test]
        fn no_slice() {
            let mut out = Vec::new();
            byte_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange::from_str("::").unwrap(),
            )
            .expect("");

            assert_eq!(
                out,
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
            );
        }

        #[test]
        fn skip_first() {
            let mut out = Vec::new();
            byte_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange::from_str("10:").unwrap(),
            )
            .expect("");

            assert_eq!(
                out,
                b"and is simple string slicing command.\nLike a python slice syntax.\n"
            );
        }

        #[test]
        fn drop_last() {
            let mut out = Vec::new();
            byte_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange::from_str(":15").unwrap(),
            )
            .expect("");

            assert_eq!(out, b"slice command i");
        }

        #[test]
        fn skip_first_and_drop_last() {
            let mut out = Vec::new();
            byte_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange::from_str("5:15").unwrap(),
            )
            .expect("");

            assert_eq!(out, b" command i");
        }

        #[test]
        fn skip_two_slice() {
            let mut out = Vec::new();
            byte_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange::from_str("::2").unwrap(),
            )
            .expect("");

            assert_eq!(out, b"siecmadi ipesrn lcn omn.Lk  yhnsiesna.");
        }
    }
}
