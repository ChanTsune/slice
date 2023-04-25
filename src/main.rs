use crate::{iterator::IteratorExt, range::SliceRange};
use clap::Parser;
use std::{
    fs,
    io::{self, stdin, stdout, Read, Write},
    path::PathBuf,
};

mod cli;
mod iterator;
mod range;

fn line_mode<R: Read, W: Write>(input: R, output: W, range: &SliceRange) -> io::Result<()> {
    let mut out = io::BufWriter::new(output);
    for line in io::BufReader::new(input)
        .lines_with_eol()
        .take(range.end)
        .skip(range.start)
        .step_by(range.step.map(|step| step.get()).unwrap_or(1))
    {
        let line = line?;
        out.write_all(line.as_bytes())?;
    }
    Ok(())
}

fn character_mode<R: Read, W: Write>(input: R, output: W, range: &SliceRange) -> io::Result<()> {
    let mut out = io::BufWriter::new(output);
    for byte in io::BufReader::new(input)
        .bytes()
        .take(range.end)
        .skip(range.start)
        .step_by(range.step.map(|step| step.get()).unwrap_or(1))
    {
        out.write_all(&[byte?])?;
    }
    Ok(())
}

fn multi<W: Write, F: Fn(fs::File, &W, &SliceRange) -> io::Result<()>>(
    targets: Vec<PathBuf>,
    mut out: W,
    range: &SliceRange,
    print_header: bool,
    f: F,
) -> io::Result<()> {
    for target in targets {
        if print_header {
            writeln!(out, "==> {} <==", target.display())?;
        }
        f(fs::File::open(target)?, &out, range)?;
    }
    Ok(())
}

fn entry(args: cli::Args) -> io::Result<()> {
    if args.files.is_empty() {
        if args.characters {
            character_mode(stdin(), stdout(), &args.range)
        } else {
            line_mode(stdin(), stdout(), &args.range)
        }
    } else if args.files.len() == 1 {
        if args.characters {
            character_mode(
                fs::File::open(args.files.first().expect(""))?,
                stdout(),
                &args.range,
            )
        } else {
            line_mode(
                fs::File::open(args.files.first().expect(""))?,
                stdout(),
                &args.range,
            )
        }
    } else {
        if args.characters {
            multi(
                args.files,
                stdout(),
                &args.range,
                !args.quiet_headers,
                |input, output, range| character_mode(input, output, range),
            )
        } else {
            multi(
                args.files,
                stdout(),
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
