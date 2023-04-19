use crate::range::SliceRange;
use clap::{value_parser, ArgGroup, Parser, Subcommand};
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
