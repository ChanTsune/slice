#![doc = include_str!("../README.md")]

mod cli;
mod ext;
mod range;

use crate::{
    ext::{slice_window, BufReadExt, Byte, Bytes, IteratorExt, PerByte, Split},
    range::SliceRange,
};
use clap::{CommandFactory, Parser};
use std::{
    fs,
    io::{self, stdin, stdout, BufRead, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

enum SliceMode<'b> {
    Lines,
    Bytes,
    Custom(&'b [u8]),
}

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

#[inline]
fn line_mode<R: BufRead, W: Write>(input: R, mut output: W, range: &SliceRange) -> io::Result<()> {
    for line in input
        .lines_with_eol()
        .slice(range.start, range.end, range.step)
    {
        output.write_all(&line?)?;
    }
    output.flush()
}

#[inline]
fn delimit_mode<R: BufRead, W: Write>(
    input: R,
    output: W,
    delimiter: &[u8],
    range: &SliceRange,
) -> io::Result<()> {
    fn run<S: Split, R: BufRead, W: Write>(
        split: S,
        input: R,
        mut output: W,
        range: &SliceRange,
    ) -> io::Result<()> {
        for part in input
            .split_chunks(split)
            .slice(range.start, range.end, range.step)
        {
            output.write_all(&part?)?;
        }
        output.flush()
    }
    match delimiter {
        [] => run(PerByte, input, output, range),
        &[b] => run(Byte(b), input, output, range),
        multi => run(Bytes(multi), input, output, range),
    }
}

#[inline]
fn delimit_window<R: BufRead, W: Write>(
    input: R,
    output: W,
    delimiter: &[u8],
    range: &SliceRange,
) -> io::Result<()> {
    match delimiter {
        [] => slice_window(PerByte, input, output, range.start, range.end),
        &[b] => slice_window(Byte(b), input, output, range.start, range.end),
        multi => slice_window(Bytes(multi), input, output, range.start, range.end),
    }
}

#[inline]
fn byte_mode<R: BufRead, W: Write>(input: R, mut output: W, range: &SliceRange) -> io::Result<()> {
    for byte in input.bytes().slice(range.start, range.end, range.step) {
        output.write_all(&[byte?])?;
    }
    output.flush()
}

// stdin and pipes are not Seek; advance past `start` bytes by consuming them.
#[inline]
fn discard<R: BufRead>(reader: &mut R, mut n: u64) -> io::Result<()> {
    while n > 0 {
        let consumed = {
            let buf = match reader.fill_buf() {
                Ok(buf) => buf,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            if buf.is_empty() {
                break;
            }
            buf.len().min(n as usize)
        };
        reader.consume(consumed);
        n -= consumed as u64;
    }
    Ok(())
}

// Byte-mode unit-step fast-path: emits input bytes [start, min(end, len)),
// matching IteratorExt::slice's take(end).skip(start) ordering — `end` is an
// absolute index from the stream start, so start >= end yields an empty window.
#[inline]
fn byte_window<R, W, S>(
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
    skip: S,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    S: Fn(&mut R, u64) -> io::Result<()>,
{
    if start > 0 {
        skip(&mut input, start as u64)?;
    }
    match end {
        Some(end) => {
            let len = end.saturating_sub(start) as u64;
            io::copy(&mut (&mut input).take(len), &mut output)?;
        }
        None => {
            io::copy(&mut input, &mut output)?;
        }
    }
    output.flush()
}

#[inline]
fn explain_mode<W: Write>(mut output: W, range: &SliceRange, unit: &str) -> io::Result<()> {
    output.write_all(range.explain(unit).as_bytes())?;
    output.flush()
}

#[inline]
fn copy_mode<R: BufRead, W: Write>(mut input: R, mut output: W) -> io::Result<()> {
    io::copy(&mut input, &mut output)?;
    output.flush()
}

#[inline]
fn apply<R, W, S>(
    mode: &SliceMode,
    input: R,
    output: W,
    range: &SliceRange,
    skip: S,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    S: Fn(&mut R, u64) -> io::Result<()>,
{
    if range.is_identity() {
        return copy_mode(input, output);
    }
    match mode {
        SliceMode::Lines => {
            if range.is_unit_step() {
                slice_window(Byte(b'\n'), input, output, range.start, range.end)
            } else {
                line_mode(input, output, range)
            }
        }
        SliceMode::Bytes => {
            if range.is_unit_step() {
                byte_window(input, output, range.start, range.end, skip)
            } else {
                byte_mode(input, output, range)
            }
        }
        SliceMode::Custom(delimiter) => {
            if range.is_unit_step() {
                delimit_window(input, output, delimiter, range)
            } else {
                delimit_mode(input, output, delimiter, range)
            }
        }
    }
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

// Exit status for output written without a file context (stdout from stdin or
// --explain): broken pipe is a quiet success, any other error is reported.
fn stdout_status(result: io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(err) if is_broken_pipe(&err) => true,
        Err(err) => {
            eprintln!("slice: {err}");
            false
        }
    }
}

#[inline]
fn multi<W: Write, R: BufRead, IW: Fn(fs::File) -> R, F: Fn(R, &mut W) -> io::Result<()>>(
    targets: &[PathBuf],
    mut out: W,
    input_wrapper: IW,
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
            f(input_wrapper(file), &mut out)
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
    let range = args.range;
    if args.explain {
        let unit = if args.bytes {
            "byte"
        } else if delimiter.is_some() {
            "part"
        } else {
            "line"
        };
        return stdout_status(explain_mode(stdout().lock(), &range, unit));
    }
    let mode = if args.bytes {
        SliceMode::Bytes
    } else if let Some(delimiter) = &delimiter {
        SliceMode::Custom(delimiter)
    } else {
        SliceMode::Lines
    };
    if args.files.is_empty() {
        let input = buf_reader(stdin().lock(), io_buffer_size);
        let output = buf_writer(stdout().lock(), io_buffer_size);
        let result = apply(&mode, input, output, &range, discard);
        stdout_status(result)
    } else {
        // A single file never gets a header, so -q only matters for 2+ files.
        let print_header = args.files.len() > 1 && !args.quiet_headers;
        let output = buf_writer(stdout().lock(), io_buffer_size);
        multi(
            &args.files,
            output,
            |input| buf_reader(input, io_buffer_size),
            print_header,
            |input, output| {
                apply(
                    &mode,
                    input,
                    output,
                    &range,
                    |r: &mut io::BufReader<fs::File>, n| r.seek(SeekFrom::Start(n)).map(drop),
                )
            },
        )
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

    mod explain {
        use super::*;

        // A writer that accepts data but fails when flushed, so a dropped
        // flush result would go unnoticed.
        struct FlushFailWriter;

        impl Write for FlushFailWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("flush failed"))
            }
        }

        #[test]
        fn writes_full_explanation() {
            let mut out = Vec::new();
            let range = SliceRange::from_str("1:3").unwrap();
            explain_mode(&mut out, &range, "line").expect("write to a Vec failed");

            assert_eq!(out, range.explain("line").into_bytes());
        }

        #[test]
        fn surfaces_flush_errors() {
            let range = SliceRange::from_str("1:3").unwrap();
            let err = explain_mode(FlushFailWriter, &range, "line")
                .expect_err("a failing flush must surface its error");

            assert_eq!(err.kind(), io::ErrorKind::Other);
        }
    }

    mod stdout_status {
        use super::*;

        #[test]
        fn broken_pipe_is_success() {
            assert!(stdout_status(Err(io::Error::from(
                io::ErrorKind::BrokenPipe
            ))));
        }

        #[test]
        fn other_errors_fail() {
            assert!(!stdout_status(Err(io::Error::other("boom"))));
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
                false,
                |input, output| line_mode(input, output, &SliceRange::from_str("::").unwrap()),
            );
            fs::remove_file(&file).ok();

            assert!(ok, "broken pipe must not fail the exit status");
        }

        #[test]
        fn explain_mode_surfaces_the_error() {
            let range = SliceRange::from_str("1:3").unwrap();
            let err = explain_mode(BrokenPipeWriter, &range, "line")
                .expect_err("a failing writer must surface its error");

            assert!(is_broken_pipe(&err));
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
                false,
                |input, output| line_mode(input, output, &SliceRange::from_str("::").unwrap()),
            );
            fs::remove_file(&readable).ok();

            assert!(!ok, "a failure before the broken pipe must be preserved");
        }

        #[test]
        fn byte_window_propagates_broken_pipe() {
            let file = temp_file(b"line one\nline two\n");
            let reader = io::BufReader::new(fs::File::open(&file).expect("open temp file"));
            let range = SliceRange::from_str("0:3").unwrap();
            let err = byte_window(
                reader,
                BrokenPipeWriter,
                range.start,
                range.end,
                |r: &mut io::BufReader<fs::File>, n| r.seek(SeekFrom::Start(n)).map(drop),
            )
            .expect_err("a broken pipe must propagate from the window path");
            fs::remove_file(&file).ok();

            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        }

        #[test]
        fn slice_window_propagates_broken_pipe() {
            let file = temp_file(b"line one\nline two\nline three\n");
            // Both window arms must surface the broken pipe: unbounded `1:`
            // exercises the io::copy tail, bounded `0:3` the write_all loop.
            for range in ["1:", "0:3"] {
                let reader = io::BufReader::new(fs::File::open(&file).expect("open temp file"));
                let range = SliceRange::from_str(range).unwrap();
                let err = slice_window(
                    Byte(b'\n'),
                    reader,
                    BrokenPipeWriter,
                    range.start,
                    range.end,
                )
                .expect_err("a broken pipe must propagate from slice_window");
                assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            }
            fs::remove_file(&file).ok();
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

    mod byte_window {
        use super::*;

        const FIXTURE: &[u8] =
            b"slice command is simple string slicing command.\nLike a python slice syntax.\n";

        fn windowed(input: &[u8], range: &str) -> Vec<u8> {
            let range = SliceRange::from_str(range).unwrap();
            let mut out = Vec::new();
            byte_window(input, &mut out, range.start, range.end, discard).expect("");
            out
        }

        #[test]
        fn skip_first() {
            assert_eq!(
                windowed(FIXTURE, "10:"),
                b"and is simple string slicing command.\nLike a python slice syntax.\n"
            );
        }

        #[test]
        fn drop_last() {
            assert_eq!(windowed(FIXTURE, ":15"), b"slice command i");
        }

        #[test]
        fn skip_first_and_drop_last() {
            assert_eq!(windowed(FIXTURE, "5:15"), b" command i");
        }

        #[test]
        fn unbounded_from_offset_no_trailing_newline() {
            assert_eq!(windowed(b"abcde", "2:"), b"cde");
        }

        #[test]
        fn start_at_or_past_end_is_empty() {
            assert_eq!(windowed(FIXTURE, "3:1"), b"");
        }

        #[test]
        fn start_past_eof_is_empty() {
            assert_eq!(windowed(FIXTURE, "200:"), b"");
        }

        #[test]
        fn bounded_window_past_eof_stops_at_eof() {
            assert_eq!(windowed(FIXTURE, "70:1000"), b"ntax.\n");
        }

        #[test]
        fn end_beyond_len_stops_at_eof() {
            assert_eq!(windowed(b"abc", "0:5"), b"abc");
        }

        #[test]
        fn empty_input() {
            assert_eq!(windowed(b"", "::"), b"");
            assert_eq!(windowed(b"", "0:5"), b"");
        }

        #[test]
        fn binary_is_preserved_byte_exact() {
            let input = b"sl\xaace\xaabinary stream without newline";
            assert_eq!(
                windowed(input, "2:"),
                b"\xaace\xaabinary stream without newline"
            );
        }
    }

    mod seek_vs_discard {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

        fn temp_file(contents: &[u8]) -> PathBuf {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "slice-seek-vs-discard-{}-{}.txt",
                std::process::id(),
                id
            ));
            fs::write(&path, contents).expect("write temp file");
            path
        }

        fn via_discard(input: &[u8], range: &SliceRange) -> Vec<u8> {
            let mut out = Vec::new();
            byte_window(input, &mut out, range.start, range.end, discard).expect("");
            out
        }

        fn via_seek(path: &Path, range: &SliceRange) -> Vec<u8> {
            let reader = io::BufReader::new(fs::File::open(path).expect("open temp file"));
            let mut out = Vec::new();
            byte_window(
                reader,
                &mut out,
                range.start,
                range.end,
                |r: &mut io::BufReader<fs::File>, n| r.seek(SeekFrom::Start(n)).map(drop),
            )
            .expect("");
            out
        }

        #[test]
        fn seek_matches_discard() {
            let input =
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n";
            let path = temp_file(input);
            for range in ["10:", "5:15", "0:4", "200:"] {
                let range = SliceRange::from_str(range).unwrap();
                assert_eq!(
                    via_seek(&path, &range),
                    via_discard(input, &range),
                    "seek and discard diverged for {range:?}"
                );
            }
            fs::remove_file(&path).ok();
        }
    }

    mod copy {
        use super::*;

        #[test]
        fn empty_input() {
            let mut out = Vec::new();
            copy_mode(b"".as_slice(), &mut out).expect("");
            assert_eq!(out, b"");
        }

        #[test]
        fn verbatim_including_binary_and_missing_eol() {
            let input = b"slice\xaabinary stream\nno trailing eol";
            let mut out = Vec::new();
            copy_mode(input.as_slice(), &mut out).expect("");
            assert_eq!(out, input);
        }
    }

    mod identity_fast_path {
        use super::*;

        const INPUT: &[u8] = b"a,b,c\nd,e,f\n";

        fn applied(mode: SliceMode, range: &str) -> Vec<u8> {
            let mut out = Vec::new();
            apply(
                &mode,
                INPUT,
                &mut out,
                &SliceRange::from_str(range).unwrap(),
                discard,
            )
            .expect("");
            out
        }

        #[test]
        fn lines_colon() {
            assert_eq!(applied(SliceMode::Lines, ":"), INPUT);
        }

        #[test]
        fn lines_colon_colon() {
            assert_eq!(applied(SliceMode::Lines, "::"), INPUT);
        }

        #[test]
        fn lines_explicit_unit_step() {
            assert_eq!(applied(SliceMode::Lines, "0::1"), INPUT);
        }

        #[test]
        fn bytes() {
            assert_eq!(applied(SliceMode::Bytes, "::"), INPUT);
        }

        #[test]
        fn custom_delimiter() {
            assert_eq!(applied(SliceMode::Custom(b",".as_slice()), "::"), INPUT);
        }

        #[test]
        fn non_identity_still_slices() {
            let mut out = Vec::new();
            apply(
                &SliceMode::Lines,
                b"a\nb\nc\n".as_slice(),
                &mut out,
                &SliceRange::from_str("1:").unwrap(),
                discard,
            )
            .expect("");
            assert_eq!(out, b"b\nc\n");
        }

        #[test]
        fn custom_unit_step_matches_delimit_mode() {
            // The unit-step Custom window (delimit_window) must agree with the
            // step>1 iterator path (delimit_mode) for every delimiter shape.
            fn agree(delimiter: &[u8], input: &[u8], range: &str) -> Vec<u8> {
                let range = SliceRange::from_str(range).unwrap();
                let mut via_apply = Vec::new();
                apply(
                    &SliceMode::Custom(delimiter),
                    input,
                    &mut via_apply,
                    &range,
                    discard,
                )
                .expect("");
                let mut via_iter = Vec::new();
                delimit_mode(input, &mut via_iter, delimiter, &range).expect("");
                assert_eq!(
                    via_apply, via_iter,
                    "apply vs delimit_mode for {delimiter:?} {range:?}"
                );
                via_apply
            }
            assert_eq!(agree(b"||", b"a||b||c\n", "1:"), b"b||c\n"); // multi-byte
            assert_eq!(agree(b",", b"a,b,c,", "1:"), b"b,c,"); // single-byte
            assert_eq!(agree(b"", b"abcdef", "1:4"), b"bcd"); // empty (PerByte)
        }
    }

    mod slice_window {
        use super::*;
        use crate::ext::Byte;

        fn windowed(input: &[u8], range: &str) -> Vec<u8> {
            let range = SliceRange::from_str(range).unwrap();
            let mut out = Vec::new();
            slice_window(Byte(b'\n'), input, &mut out, range.start, range.end).expect("");
            out
        }

        #[test]
        fn unbounded_from_offset() {
            assert_eq!(windowed(b"a\nb\nc\n", "1:"), b"b\nc\n");
        }

        #[test]
        fn bounded_drop_tail() {
            assert_eq!(windowed(b"a\nb\nc\n", ":1"), b"a\n");
        }

        #[test]
        fn unbounded_last_line_without_newline() {
            assert_eq!(windowed(b"a\nb", "1:"), b"b");
        }

        #[test]
        fn empty_bounded_window() {
            assert_eq!(windowed(b"a\nb\nc\n", ":0"), b"");
        }

        #[test]
        fn skip_past_eof_is_empty() {
            assert_eq!(windowed(b"a\nb\n", "2:"), b"");
        }

        #[test]
        fn empty_input() {
            assert_eq!(windowed(b"", "::"), b"");
            assert_eq!(windowed(b"", "1:"), b"");
            assert_eq!(windowed(b"", "0:3"), b"");
        }
    }
}
