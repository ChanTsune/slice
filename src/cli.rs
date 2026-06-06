use crate::range::SliceRange;
use bytesize::ByteSize;
use clap::{ArgGroup, Parser};
use std::{path::PathBuf, str::FromStr};

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct NonZeroByteSize(ByteSize);

impl FromStr for NonZeroByteSize {
    type Err = <ByteSize as FromStr>::Err;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bs = ByteSize::from_str(s)?;
        if bs.0 == 0 {
            Err(Self::Err::from("0 is not allowed"))
        } else {
            Ok(Self(bs))
        }
    }
}

#[derive(Parser, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version,
    about,
    author,
    arg_required_else_help = true,
    group(ArgGroup::new("mode").args(["lines", "bytes", "delimiter", "null"])),
)]
pub(crate) struct Args {
    #[arg(
        help = "The slice syntax is similar to Python's slice syntax, with the format `start:end:step`.
Each value is optional and, if omitted, defaults to the start of the file, the end of the file, and a step of 1, respectively.
e.g., '50:100', '50:100:1'
and the extended syntax 'start:+line' is supported. (experimental)
e.g., '50:+50'"
    )]
    pub(crate) range: SliceRange,
    #[arg(short, help = "Slice the lines (default)")]
    pub(crate) lines: bool,
    // `-c` is a hidden short alias kept for backward compatibility.
    #[arg(short, long, short_alias = 'c', help = "Slice the bytes")]
    pub(crate) bytes: bool,
    #[arg(long, help = "Slice by delimiter")]
    pub(crate) delimiter: Option<String>,
    #[arg(short = 'z', long = "null", help = "Use NUL (\\0) as the delimiter")]
    pub(crate) null: bool,
    #[arg(
        short = 'e',
        long = "escape",
        requires = "delimiter",
        help = "Interpret backslash escapes in --delimiter (\\t \\n \\r \\0 \\\\ \\xHH)"
    )]
    pub(crate) escape: bool,
    #[arg(
        long,
        help = "Explain what the range selects and exit without reading input. Any FILES are ignored"
    )]
    pub(crate) explain: bool,
    #[arg(
        short,
        help = "Suppresses printing of headers when multiple files are being examined"
    )]
    pub(crate) quiet_headers: bool,
    #[arg(
        long,
        help = "Set the size of the I/O buffer. This buffer is used for both input and output operations (experimental)"
    )]
    pub(crate) io_buffer_size: Option<NonZeroByteSize>,
    #[arg(help = "Target files. if not provided use stdin")]
    pub(crate) files: Vec<PathBuf>,
}

impl Args {
    #[inline]
    pub(crate) fn io_buffer_size(&self) -> Option<usize> {
        self.io_buffer_size.map(|it| it.0.as_u64() as usize)
    }

    /// Resolve the effective delimiter bytes. `--null` yields a single NUL
    /// byte; otherwise `--delimiter` is taken literally unless `--escape` is
    /// set, in which case backslash escapes are expanded.
    pub(crate) fn delimiter(&self) -> Result<Option<Vec<u8>>, String> {
        if self.null {
            return Ok(Some(vec![0]));
        }
        match &self.delimiter {
            Some(s) if self.escape => unescape(s).map(Some),
            Some(s) => Ok(Some(s.clone().into_bytes())),
            None => Ok(None),
        }
    }
}

/// Expand C-style backslash escapes (`\t \n \r \0 \\ \xHH`) into raw bytes.
/// Non-escaped bytes pass through unchanged, so UTF-8 delimiters are preserved.
fn unescape(s: &str) -> Result<Vec<u8>, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'\\' {
            out.push(b);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&esc) = bytes.get(i) else {
            return Err("trailing backslash in delimiter".to_owned());
        };
        match esc {
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'0' => out.push(0),
            b'\\' => out.push(b'\\'),
            b'x' => {
                let (Some(&hi), Some(&lo)) = (bytes.get(i + 1), bytes.get(i + 2)) else {
                    return Err("`\\x` needs two hex digits".to_owned());
                };
                let (Some(h), Some(l)) = ((hi as char).to_digit(16), (lo as char).to_digit(16))
                else {
                    return Err(format!(
                        "invalid hex escape `\\x{}{}`",
                        hi as char, lo as char
                    ));
                };
                out.push((h << 4 | l) as u8);
                i += 2;
            }
            other => return Err(format!("unknown escape sequence `\\{}`", other as char)),
        }
        i += 1;
    }
    Ok(out)
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
                end: None,
                step: NonZeroUsize::new(1),
            }
        );
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }

    #[test]
    fn byte_mode_args() {
        let args = Args::parse_from(["slice", "-b", "0::1", "text.txt"]);
        assert!(args.bytes);
        assert_eq!(
            args.range,
            SliceRange {
                start: 0,
                end: None,
                step: NonZeroUsize::new(1),
            }
        );
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }

    #[test]
    fn long_bytes_flag_args() {
        let args = Args::parse_from(["slice", "--bytes", "0::1", "text.txt"]);
        assert!(args.bytes);
    }

    #[test]
    fn c_short_alias_sets_bytes() {
        let args = Args::parse_from(["slice", "-c", "0::1", "text.txt"]);
        assert!(args.bytes);
    }

    #[test]
    fn explain_flag_parses() {
        let args = Args::parse_from(["slice", "--explain", "10:20"]);
        assert!(args.explain);
        assert_eq!(
            args.range,
            SliceRange {
                start: 10,
                end: Some(20),
                step: None,
            }
        );
    }

    #[test]
    fn explain_help_mentions_without_reading_input() {
        use clap::CommandFactory;
        let cmd = Args::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "explain")
            .expect("explain arg");
        let help = arg.get_help().expect("help text").to_string();
        assert!(help.to_lowercase().contains("without reading input"));
    }

    #[test]
    fn bytes_help_mentions_bytes() {
        use clap::CommandFactory;
        let cmd = Args::command();
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_id().as_str() == "bytes")
            .expect("bytes arg");
        let help = arg.get_help().expect("help text").to_string();
        assert!(help.to_lowercase().contains("byte"));
    }

    #[test]
    fn delimiter_null() {
        let args = Args::parse_from(["slice", "-z", "0:"]);
        assert_eq!(args.delimiter().unwrap(), Some(vec![0]));
    }

    #[test]
    fn delimiter_passthrough() {
        let args = Args::parse_from(["slice", "--delimiter", ",", "0:"]);
        assert_eq!(args.delimiter().unwrap(), Some(b",".to_vec()));
    }

    #[test]
    fn unescape_basic() {
        assert_eq!(unescape("\\t").unwrap(), b"\t");
        assert_eq!(unescape("\\n").unwrap(), b"\n");
        assert_eq!(unescape("\\r\\n").unwrap(), b"\r\n");
        assert_eq!(unescape("\\0").unwrap(), [0]);
        assert_eq!(unescape("\\\\").unwrap(), b"\\");
        assert_eq!(unescape(",").unwrap(), b",");
        assert_eq!(unescape("a\\tb").unwrap(), b"a\tb");
    }

    #[test]
    fn unescape_hex() {
        assert_eq!(unescape("\\x41").unwrap(), [0x41]);
        assert_eq!(unescape("\\xff").unwrap(), [0xff]);
        assert_eq!(unescape("\\x00\\x01").unwrap(), [0x00, 0x01]);
    }

    #[test]
    fn unescape_errors() {
        assert!(unescape("\\q").is_err());
        assert!(unescape("trailing\\").is_err());
        assert!(unescape("\\x").is_err());
        assert!(unescape("\\xZZ").is_err());
    }

    #[test]
    fn delimiter_literal_by_default() {
        let args = Args::parse_from(["slice", "--delimiter", "\\t", "0:"]);
        assert_eq!(args.delimiter().unwrap(), Some(b"\\t".to_vec()));
    }

    #[test]
    fn delimiter_escaped_with_flag() {
        let args = Args::parse_from(["slice", "--delimiter", "\\t", "-e", "0:"]);
        assert_eq!(args.delimiter().unwrap(), Some(b"\t".to_vec()));
    }

    #[test]
    fn delimiter_none() {
        let args = Args::parse_from(["slice", "0:"]);
        assert_eq!(args.delimiter().unwrap(), None);
    }

    #[test]
    fn mode_flags_are_mutually_exclusive() {
        assert!(Args::try_parse_from(["slice", "-z", "-b", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "-z", "--delimiter", ",", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "-b", "--delimiter", ",", "0:"]).is_err());
    }

    #[test]
    fn escape_requires_delimiter() {
        assert!(Args::try_parse_from(["slice", "-e", "0:"]).is_err());
    }
}
