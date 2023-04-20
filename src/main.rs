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

    #[test]
    fn line_empty() {
        let mut out = Vec::new();
        line_mode(
            "".as_bytes(),
            out.as_mut_slice(),
            &SliceRange {
                start: 0,
                end: 0,
                step: None,
            },
        )
        .unwrap();

        assert_eq!(std::str::from_utf8(&out).expect(""), "")
    }

    #[test]
    fn character_empty() {
        let mut out = Vec::new();
        character_mode(
            "".as_bytes(),
            out.as_mut_slice(),
            &SliceRange {
                start: 0,
                end: 0,
                step: None,
            },
        )
        .unwrap();

        assert_eq!(std::str::from_utf8(&out).expect(""), "")
    }
}
