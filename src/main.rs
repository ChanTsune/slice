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

#[inline]
fn byte_mode<R: BufRead, W: Write>(input: R, mut output: W, range: &SliceRange) -> io::Result<()> {
    for byte in input.bytes().slice(range.start, range.end, range.step) {
        output.write_all(&[byte?])?;
    }
    output.flush()
}

fn report_error(path: &Path, err: &io::Error) {
    eprintln!("slice: {}: {}", path.display(), err);
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
        if print_header {
            if let Err(err) = writeln!(out, "==> {} <==", target.display()) {
                report_error(target, &err);
                ok = false;
                continue;
            }
        }
        if let Err(err) = f(input_wrapper(file), &mut out, range) {
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
