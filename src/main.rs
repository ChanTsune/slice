use crate::range::SliceRange;
use clap::Parser;
use std::{
    fs,
    io::{self, stdin, stdout, BufRead, Read, Write},
    path::PathBuf,
};

mod cli;
mod range;

fn line_mode<R: Read, W: Write>(input: R, output: W, range: &SliceRange) -> io::Result<()> {
    let mut out = io::BufWriter::new(output);
    for (idx, line) in io::BufReader::new(input)
        .lines()
        .enumerate()
        .skip(range.start)
        .step_by(range.step.map(|step| step.get()).unwrap_or(1))
    {
        if range.end <= idx {
            break;
        }
        let line = line?;
        out.write_all(line.as_bytes())?;
        out.write_all(b"\n")?;
    }
    Ok(())
}

fn character_mode<R: Read, W: Write>(input: R, output: W, range: &SliceRange) -> io::Result<()> {
    let mut out = io::BufWriter::new(output);
    for (idx, byte) in io::BufReader::new(input)
        .bytes()
        .enumerate()
        .skip(range.start)
        .step_by(range.step.map(|step| step.get()).unwrap_or(1))
    {
        if range.end <= idx {
            break;
        }
        out.write_all(&[byte?])?;
    }
    Ok(())
}

fn multi<W: Write, F: Fn(fs::File, &W, &SliceRange) -> io::Result<()>>(
    targets: Vec<PathBuf>,
    mut out: W,
    range: &SliceRange,
    f: F,
) -> io::Result<()> {
    for target in targets {
        writeln!(out, "==> {} <==", target.display())?;
        f(fs::File::open(target)?, &out, range)?;
    }
    Ok(())
}

fn entry(args: cli::Cli) -> io::Result<()> {
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
            multi(args.files, stdout(), &args.range, |input, output, range| {
                character_mode(input, output, range)
            })
        } else {
            multi(args.files, stdout(), &args.range, |input, output, range| {
                line_mode(input, output, range)
            })
        }
    }
}

fn main() -> io::Result<()> {
    entry(cli::Cli::parse())
}

#[cfg(test)]
mod tests {
    use super::*;

    mod line {
        use super::*;

        #[test]
        fn empty() {
            let mut out = Vec::new();
            line_mode(
                b"".as_slice(),
                &mut out,
                &SliceRange {
                    start: 0,
                    end: usize::MAX,
                    step: None,
                },
            )
            .expect("");

            assert_eq!(out, b"");
        }

        mod one_line {
            use super::*;
            use std::num::NonZeroUsize;

            #[test]
            fn no_slice() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\n".as_slice(),
                    &mut out,
                    &SliceRange {
                        start: 0,
                        end: usize::MAX,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 1,
                        end: usize::MAX,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 2,
                        end: usize::MAX,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 0,
                        end: 0,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 0,
                        end: usize::MAX,
                        step: NonZeroUsize::new(2),
                    },
                )
                .expect("");

                assert_eq!(out, b"slice command is simple string slicing command.\n");
            }
        }

        mod multi_line {
            use super::*;
            use std::num::NonZeroUsize;

            #[test]
            fn no_slice() {
                let mut out = Vec::new();
                line_mode(
                    b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                        .as_slice(),
                    &mut out,
                    &SliceRange {
                        start: 0,
                        end: usize::MAX,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 1,
                        end: usize::MAX,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 0,
                        end: 1,
                        step: None,
                    },
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
                    &SliceRange {
                        start: 0,
                        end: usize::MAX,
                        step: NonZeroUsize::new(2),
                    },
                )
                    .expect("");

                assert_eq!(
                    out,
                    b"slice command is simple string slicing command.\n".repeat(5)
                );
            }
        }
    }

    mod character {
        use std::num::NonZeroUsize;
        use super::*;

        #[test]
        fn empty() {
            let mut out = Vec::new();
            character_mode(
                b"".as_slice(),
                &mut out,
                &SliceRange {
                    start: 0,
                    end: usize::MAX,
                    step: None,
                },
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
                &SliceRange {
                    start: 0,
                    end: usize::MAX,
                    step: None,
                },
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
                &SliceRange {
                    start: 10,
                    end: usize::MAX,
                    step: None,
                },
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
                &SliceRange {
                    start: 0,
                    end: 15,
                    step: None,
                },
            )
                .expect("");

            assert_eq!(
                out,
                b"slice command i"
            );
        }

        #[test]
        fn skip_first_and_drop_last() {
            let mut out = Vec::new();
            character_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange {
                    start: 5,
                    end: 15,
                    step: None,
                },
            )
                .expect("");

            assert_eq!(
                out,
                b" command i"
            );
        }

        #[test]
        fn skip_two_slice() {
            let mut out = Vec::new();
            character_mode(
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n"
                    .as_slice(),
                &mut out,
                &SliceRange {
                    start: 0,
                    end: usize::MAX,
                    step: NonZeroUsize::new(2),
                },
            )
                .expect("");

            assert_eq!(
                out,
                b"siecmadi ipesrn lcn omn.Lk  yhnsiesna."
            );
        }

    }
}
