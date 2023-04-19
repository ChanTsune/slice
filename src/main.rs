use crate::range::SliceRange;
use clap::Parser;
use std::io::{stdin, stdout, BufRead, Read, Seek, Write};
use std::path::PathBuf;
use std::{fs, io};

mod cli;
mod range;

fn single_file<R: Read>(input: R, range: &SliceRange) -> io::Result<()> {
    let mut out = io::BufWriter::new(stdout());
    for (idx, line) in io::BufReader::new(input)
        .lines()
        .enumerate()
        .skip(range.start)
        .step_by(range.step)
    {
        if range.end <= idx {
            break;
        }
        let line = line?;
        out.write_all(line.as_bytes())?;
        out.write(b"\n")?;
    }
    Ok(())
}

fn multi(targets: Vec<PathBuf>, range: &SliceRange) -> io::Result<()> {
    for target in targets {
        single_file(fs::File::open(target)?, range)?;
    }
    Ok(())
}

fn entry(args: cli::Cli) -> io::Result<()> {
    if args.files.is_empty() {
        single_file(stdin(), &args.range)
    } else if args.files.len() == 1 {
        single_file(fs::File::open(args.files.first().expect(""))?, &args.range)
    } else {
        multi(args.files, &args.range)
    }
}

fn main() -> io::Result<()> {
    entry(cli::Cli::parse())
}
