mod cli;
mod ext;
mod range;

use crate::{
    ext::{BufReadExt, IteratorExt},
    range::SliceRange,
};
use clap::Parser;
use std::{
    fs,
    io::{self, stdin, stdout, BufRead, Read, Write},
    path::PathBuf,
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
fn character_mode<R: BufRead, W: Write>(
    input: R,
    mut output: W,
    range: &SliceRange,
) -> io::Result<()> {
    for byte in input.bytes().slice(range.start, range.end, range.step) {
        output.write_all(&[byte?])?;
    }
    output.flush()
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
) -> io::Result<()> {
    for target in targets {
        if print_header {
            writeln!(out, "==> {} <==", target.display())?;
        }
        let reader = input_wrapper(fs::File::open(target)?);
        f(reader, &mut out, range)?;
    }
    Ok(())
}

fn entry(args: cli::Args) -> io::Result<()> {
    if args.files.is_empty() {
        let input = buf_reader(stdin().lock(), args.io_buffer_size());
        let output = buf_writer(stdout().lock(), args.io_buffer_size());
        if args.characters {
            character_mode(input, output, &args.range)
        } else if let Some(delimiter) = args.delimiter {
            delimit_mode(input, output, delimiter.as_bytes(), &args.range)
        } else {
            line_mode(input, output, &args.range)
        }
    } else if args.files.len() == 1 {
        let input = buf_reader(fs::File::open(&args.files[0])?, args.io_buffer_size());
        let output = buf_writer(stdout().lock(), args.io_buffer_size());
        if args.characters {
            character_mode(input, output, &args.range)
        } else if let Some(delimiter) = args.delimiter {
            delimit_mode(input, output, delimiter.as_bytes(), &args.range)
        } else {
            line_mode(input, output, &args.range)
        }
    } else {
        let io_buffer_size = args.io_buffer_size();
        let output = buf_writer(stdout().lock(), io_buffer_size);
        if args.characters {
            multi(
                &args.files,
                output,
                |input| buf_reader(input, io_buffer_size),
                &args.range,
                !args.quiet_headers,
                |input, output, range| character_mode(input, output, range),
            )
        } else if let Some(delimiter) = args.delimiter {
            multi(
                &args.files,
                output,
                |input| buf_reader(input, io_buffer_size),
                &args.range,
                !args.quiet_headers,
                |input, output, range| delimit_mode(input, output, delimiter.as_bytes(), range),
            )
        } else {
            multi(
                &args.files,
                output,
                |input| buf_reader(input, io_buffer_size),
                &args.range,
                !args.quiet_headers,
                |input, output, range| line_mode(input, output, range),
            )
        }
    }
}

fn main() -> io::Result<()> {
    entry(cli::Args::parse())
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

    mod character {
        use super::*;

        #[test]
        fn empty() {
            let mut out = Vec::new();
            character_mode(
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
            character_mode(
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
            character_mode(
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
            character_mode(
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
            character_mode(
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
            character_mode(
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
