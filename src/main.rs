#![doc = include_str!("../README.md")]

mod cli;
mod ext;
mod range;

use crate::{
    ext::{BufReadExt, IteratorExt},
    range::SliceRange,
};
use clap::{CommandFactory, Parser};
use std::{
    fs,
    io::{self, stdin, stdout, BufRead, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
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

/// Index just past the `need`-th occurrence of `delim` in `buf`, together with
/// the number of occurrences found (capped at `need`).
#[inline]
fn scan_delims(buf: &[u8], delim: u8, need: usize) -> (usize, usize) {
    let mut found = 0;
    for i in memchr::memchr_iter(delim, buf) {
        found += 1;
        if found == need {
            return (i + 1, found);
        }
    }
    (buf.len(), found)
}

/// Whether item `i` is selected by `[start:end:step]`.
#[inline]
fn selected(i: usize, start: usize, end: usize, step: usize) -> bool {
    i >= start && i < end && (i - start) % step == 0
}

/// Advance `input` past `start` bytes. Returns `false` if the stream ended
/// before that many bytes were available.
#[inline]
fn skip_bytes<R: BufRead>(input: &mut R, start: usize) -> io::Result<bool> {
    let mut pos = 0;
    while pos < start {
        let n = {
            let buf = input.fill_buf()?;
            if buf.is_empty() {
                return Ok(false);
            }
            buf.len().min(start - pos)
        };
        input.consume(n);
        pos += n;
    }
    Ok(true)
}

/// Copy the byte span `[start, end)` of the stream in bulk.
#[inline]
fn byte_range<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    start: usize,
    end: usize,
) -> io::Result<()> {
    if !skip_bytes(input, start)? {
        return Ok(());
    }
    if end == usize::MAX {
        return io::copy(input, output).map(drop);
    }
    let mut pos = start;
    while pos < end {
        let n = {
            let buf = input.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let take = buf.len().min(end - pos);
            output.write_all(&buf[..take])?;
            take
        };
        input.consume(n);
        pos += n;
    }
    Ok(())
}

/// Emit the bytes `start, start + step, ...` that fall within `[start, end)`.
#[inline]
fn byte_stepped<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    start: usize,
    end: usize,
    step: usize,
) -> io::Result<()> {
    if !skip_bytes(input, start)? {
        return Ok(());
    }
    let mut pos = start;
    let mut next = start;
    let mut scratch = Vec::new();
    while next < end {
        let n = {
            let buf = input.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let base = next - pos;
            if base < buf.len() {
                let limit = buf.len().min(end - pos);
                scratch.clear();
                scratch.extend(buf[base..limit].iter().step_by(step).copied());
                output.write_all(&scratch)?;
                next += scratch.len() * step;
            }
            buf.len()
        };
        input.consume(n);
        pos += n;
    }
    Ok(())
}

/// Consume `count` records terminated by `delim` (the delimiter is kept on each
/// record), writing them to `output` when it is `Some`. Returns `false` if the
/// stream ended before `count` records were seen.
#[inline]
fn consume_records<R: BufRead, W: Write>(
    input: &mut R,
    mut output: Option<&mut W>,
    delim: u8,
    mut count: usize,
) -> io::Result<bool> {
    while count > 0 {
        let n = {
            let buf = input.fill_buf()?;
            if buf.is_empty() {
                return Ok(false);
            }
            let (cut, found) = scan_delims(buf, delim, count);
            if let Some(out) = output.as_deref_mut() {
                out.write_all(&buf[..cut])?;
            }
            count -= found;
            cut
        };
        input.consume(n);
    }
    Ok(true)
}

/// Slice records terminated by the single byte `delim` (the delimiter is kept
/// on each record) for step 1: the result is the contiguous byte span from the
/// start of record `start` through the end of record `end - 1`.
#[inline]
fn record_range<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    delim: u8,
    start: usize,
    end: usize,
) -> io::Result<()> {
    if end <= start {
        return Ok(());
    }
    if !consume_records(input, None::<&mut W>, delim, start)? {
        return Ok(());
    }
    if end == usize::MAX {
        return io::copy(input, output).map(drop);
    }
    consume_records(input, Some(output), delim, end - start)?;
    Ok(())
}

/// Slice records terminated by `delim` with a step greater than 1, writing the
/// selected records directly from the read buffer without per-record allocation.
#[inline]
fn record_stepped<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    delim: u8,
    start: usize,
    end: usize,
    step: usize,
) -> io::Result<()> {
    let mut index = 0;
    let mut keep = selected(0, start, end, step);
    while index < end {
        let (n, boundary) = {
            let buf = input.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            match memchr::memchr(delim, buf) {
                Some(p) => {
                    if keep {
                        output.write_all(&buf[..=p])?;
                    }
                    (p + 1, true)
                }
                None => {
                    if keep {
                        output.write_all(buf)?;
                    }
                    (buf.len(), false)
                }
            }
        };
        input.consume(n);
        if boundary {
            index += 1;
            keep = selected(index, start, end, step);
        }
    }
    Ok(())
}

#[inline]
fn record_mode<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    delim: u8,
    range: &SliceRange,
) -> io::Result<()> {
    match range.step {
        Some(step) if step.get() > 1 => record_stepped(
            &mut input,
            &mut output,
            delim,
            range.start,
            range.end,
            step.get(),
        )?,
        _ => record_range(&mut input, &mut output, delim, range.start, range.end)?,
    }
    output.flush()
}

#[inline]
fn line_mode<R: BufRead, W: Write>(input: R, output: W, range: &SliceRange) -> io::Result<()> {
    record_mode(input, output, b'\n', range)
}

#[inline]
fn delimit_mode<R: BufRead, W: Write>(
    input: R,
    mut output: W,
    delimiter: &[u8],
    range: &SliceRange,
) -> io::Result<()> {
    match delimiter {
        // An empty delimiter degrades to per-byte records, i.e. byte mode.
        [] => byte_mode(input, output, range),
        [delim] => record_mode(input, output, *delim, range),
        // Multi-byte delimiters keep the generic split path.
        _ => {
            for part in input
                .delimit_by(delimiter)
                .slice(range.start, range.end, range.step)
            {
                output.write_all(&part?)?;
            }
            output.flush()
        }
    }
}

#[inline]
fn byte_mode<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    range: &SliceRange,
) -> io::Result<()> {
    match range.step {
        Some(step) if step.get() > 1 => {
            byte_stepped(&mut input, &mut output, range.start, range.end, step.get())?
        }
        _ => byte_range(&mut input, &mut output, range.start, range.end)?,
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

fn entry(args: cli::Args) -> bool {
    let io_buffer_size = args.io_buffer_size();
    let delimiter = match args.delimiter() {
        Ok(delimiter) => delimiter,
        Err(e) => cli::Args::command()
            .error(clap::error::ErrorKind::ValueValidation, e)
            .exit(),
    };
    if args.explain {
        let unit = if args.bytes {
            "byte"
        } else if delimiter.is_some() {
            "part"
        } else {
            "line"
        };
        print!("{}", args.range.explain(unit));
        return true;
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
                return true;
            }
            eprintln!("slice: {err}");
            return false;
        }
        true
    } else {
        // A single file never gets a header, so -q only matters for 2+ files.
        let print_header = args.files.len() > 1 && !args.quiet_headers;
        let output = buf_writer(stdout().lock(), io_buffer_size);
        if args.bytes {
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
        }
    }
}

fn main() -> ExitCode {
    if entry(cli::Args::parse()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
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
