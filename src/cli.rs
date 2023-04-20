use crate::range::SliceRange;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version,
    about,
    author,
    arg_required_else_help = true,
)]
pub(crate) struct Cli {
    #[arg(help = "Slice pattern eg. '1:100'")]
    pub(crate) range: SliceRange,
    #[arg(short, help = "Line mode")]
    pub(crate) lines: bool,
    #[arg(short, help = "Character mode")]
    pub(crate) characters: bool,
    #[arg(help = "Target files. if not provided use stdin")]
    pub(crate) files: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;

    #[test]
    fn line_mode_args() {
        let args = Cli::parse_from(["slice", "-l", "0::1", "text.txt"]);
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
        let args = Cli::parse_from(["slice", "-c", "0::1", "text.txt"]);
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
