use crate::range::SliceRange;
use clap::{ArgGroup, Parser};
use std::path::PathBuf;

#[derive(Parser, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version,
    about,
    author,
    arg_required_else_help = true,
    group(ArgGroup::new("mode").args(["lines", "characters"])),
)]
pub(crate) struct Args {
    #[arg(
        help = "The slice syntax is similar to Python's slice syntax, with the format `start:end:step`. Each value is optional and, if omitted, defaults to the beginning of the file, the end of the file, and a step of 1, respectively.
 eg. '1:100:1'"
    )]
    pub(crate) range: SliceRange,
    #[arg(short, help = "Slice the lines (default)")]
    pub(crate) lines: bool,
    #[arg(short, help = "Slice the characters")]
    pub(crate) characters: bool,
    #[arg(
        short,
        help = "Suppresses printing of headers when multiple files are being examined"
    )]
    pub(crate) quiet_headers: bool,
    #[arg(
        long,
        help = "Set the size of the I/O buffer. This buffer is used for both input and output operations (experimental)"
    )]
    pub(crate) io_buffer_size: Option<usize>,
    #[arg(help = "Target files. if not provided use stdin")]
    pub(crate) files: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    #[test]
    fn line_mode_args() {
        let args = Args::parse_from(["slice", "-l", "0::1", "text.txt"]);
        assert!(args.lines);
        assert_eq!(
            args.range,
            SliceRange {
                start: 0,
                end: usize::MAX,
                step: NonZeroUsize::new(1),
            }
        );
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }

    #[test]
    fn character_mode_args() {
        let args = Args::parse_from(["slice", "-c", "0::1", "text.txt"]);
        assert!(args.characters);
        assert_eq!(
            args.range,
            SliceRange {
                start: 0,
                end: usize::MAX,
                step: NonZeroUsize::new(1),
            }
        );
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }
}
