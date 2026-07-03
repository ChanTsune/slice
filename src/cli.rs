use crate::range::{SliceRange, TranslateDialect};
use bytesize::ByteSize;
use clap::{ArgGroup, Parser, ValueEnum};
use std::{num::NonZeroUsize, path::PathBuf, str::FromStr};

// `CompletePowershell` (not `CompletePowerShell`) so the kebab-cased value is
// `complete-powershell` rather than `complete-power-shell`.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, ValueEnum)]
pub(crate) enum Generate {
    CompleteBash,
    CompleteZsh,
    CompleteFish,
    CompletePowershell,
    Man,
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct NonZeroByteSize(NonZeroUsize);

impl FromStr for NonZeroByteSize {
    type Err = String;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bs = ByteSize::from_str(s).map_err(|err| err.to_string())?;
        let size =
            usize::try_from(bs.0).map_err(|_| "size is too large for this platform".to_owned())?;
        let size = NonZeroUsize::new(size).ok_or_else(|| "0 is not allowed".to_owned())?;
        Ok(Self(size))
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) enum MaxRecordSize {
    Unlimited,
    Limited(usize),
}

impl FromStr for MaxRecordSize {
    type Err = String;

    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("unlimited") {
            return Ok(Self::Unlimited);
        }
        let bs = ByteSize::from_str(s).map_err(|err| err.to_string())?;
        if bs.0 == 0 {
            return Err("0 is not allowed".to_owned());
        }
        let limit =
            usize::try_from(bs.0).map_err(|_| "size is too large for this platform".to_owned())?;
        Ok(Self::Limited(limit))
    }
}

#[derive(Parser, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version,
    about,
    author,
    arg_required_else_help = true,
    group(ArgGroup::new("mode").args(["lines", "bytes", "chars", "graphemes", "delimiter", "null"])),
    // --explain and --translate are both read-and-exit actions handled in
    // precedence order by entry(); group them so clap rejects both at once
    // rather than silently running one. (--generate is `exclusive`, so it
    // already conflicts with everything.)
    group(ArgGroup::new("action").args(["explain", "translate"])),
)]
pub(crate) struct Args {
    // `allow_hyphen_values` is required so tail-relative ranges (`-5:`)
    // survive flag parsing; the trade-off is that an unknown flag in range
    // position is reported as an invalid <RANGE> value (still exit 2).
    // The field is an Option with an explicit `required = true`: clap skips
    // required-argument validation when an `exclusive` arg (`--generate`) is
    // present, and the explicit requiredness keeps `<RANGE>` (not `[RANGE]`)
    // in the usage line.
    #[arg(
        allow_hyphen_values = true,
        required = true,
        help = "The slice syntax is similar to Python's slice syntax, with the format `start:end:step`.
Each value is optional and, if omitted, defaults to the start of the file, the end of the file, and a step of 1, respectively.
Negative start/end values count back from the end of the input, like Python.
A negative step selects in reverse, like Python ('::-1' reverses the input); it buffers the whole input in memory.
e.g., '50:100', '50:100:1', '-5:', '::-1'
and the extended syntax 'start:+line' is supported. (experimental)
e.g., '50:+50'"
    )]
    pub(crate) range: Option<SliceRange>,
    #[arg(short, help = "Slice the lines (default)")]
    pub(crate) lines: bool,
    // `-c` is a hidden short alias kept for backward compatibility.
    #[arg(short, long, short_alias = 'c', help = "Slice the bytes")]
    pub(crate) bytes: bool,
    // Long-only: `-c` historically means bytes, so a chars shorthand would
    // invite the same confusion cut(1) has.
    #[arg(long, help = "Slice the UTF-8 characters (code points)")]
    pub(crate) chars: bool,
    #[arg(long, help = "Slice the user-perceived characters (grapheme clusters)")]
    pub(crate) graphemes: bool,
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
    // `require_equals` forces the `--translate=<DIALECT>` spelling so a bare
    // `--translate` never swallows the following `<RANGE>` as its value; the
    // bare form then falls back to `default_missing_value`, set per build target
    // via `cfg_attr` to the platform's native toolset.
    #[arg(
        long,
        value_name = "DIALECT",
        num_args = 0..=1,
        require_equals = true,
        help = "Print the equivalent shell command for the range and mode, then exit without reading input. With no value the build target's native dialect is used. Any FILES are ignored"
    )]
    #[cfg_attr(
        all(target_os = "linux", target_env = "gnu"),
        arg(default_missing_value = "gnu")
    )]
    #[cfg_attr(
        any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ),
        arg(default_missing_value = "bsd")
    )]
    #[cfg_attr(
        not(any(
            all(target_os = "linux", target_env = "gnu"),
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )),
        arg(default_missing_value = "posix")
    )]
    pub(crate) translate: Option<TranslateDialect>,
    #[arg(
        long,
        value_name = "KIND",
        exclusive = true,
        help = "Generate the shell completion script or man page and exit without reading input"
    )]
    pub(crate) generate: Option<Generate>,
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
    #[arg(
        long,
        value_name = "SIZE|unlimited",
        help = "Maximum bytes retained for one line, custom-delimited record, or grapheme cluster in tail-relative and reverse ranges. Defaults to unlimited"
    )]
    pub(crate) max_record_size: Option<MaxRecordSize>,
    #[arg(help = "Target files. if not provided use stdin")]
    pub(crate) files: Vec<PathBuf>,
}

impl Args {
    #[inline]
    pub(crate) fn io_buffer_size(&self) -> Option<NonZeroUsize> {
        self.io_buffer_size.map(|it| it.0)
    }

    #[inline]
    pub(crate) fn max_record_size(&self) -> Option<usize> {
        match self.max_record_size {
            Some(MaxRecordSize::Limited(limit)) => Some(limit),
            Some(MaxRecordSize::Unlimited) | None => None,
        }
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
    use crate::range::{SliceIndex, Step};
    use std::num::NonZeroUsize;

    #[test]
    fn line_mode_args() {
        let args = Args::parse_from(["slice", "-l", "0::1", "text.txt"]);
        assert!(args.lines);
        assert_eq!(
            args.range,
            Some(SliceRange {
                start: SliceIndex::FromStart(0),
                end: None,
                step: Step::forward(1),
            })
        );
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }

    #[test]
    fn byte_mode_args() {
        let args = Args::parse_from(["slice", "-b", "0::1", "text.txt"]);
        assert!(args.bytes);
        assert_eq!(
            args.range,
            Some(SliceRange {
                start: SliceIndex::FromStart(0),
                end: None,
                step: Step::forward(1),
            })
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
    fn chars_flag_parses() {
        let args = Args::parse_from(["slice", "--chars", "0:5", "text.txt"]);
        assert!(args.chars);
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }

    #[test]
    fn chars_conflicts_with_other_modes() {
        assert!(Args::try_parse_from(["slice", "--chars", "-b", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--chars", "-c", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--chars", "-l", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--chars", "-z", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--chars", "--delimiter", ",", "0:"]).is_err());
    }

    #[test]
    fn graphemes_flag_parses() {
        let args = Args::parse_from(["slice", "--graphemes", "0:5", "text.txt"]);
        assert!(args.graphemes);
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);
    }

    #[test]
    fn graphemes_conflicts_with_other_modes() {
        assert!(Args::try_parse_from(["slice", "--graphemes", "-b", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--graphemes", "-c", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--graphemes", "-l", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--graphemes", "-z", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--graphemes", "--chars", "0:"]).is_err());
        assert!(Args::try_parse_from(["slice", "--graphemes", "--delimiter", ",", "0:"]).is_err());
    }

    #[test]
    fn negative_range_survives_flag_parsing() {
        let tail = SliceRange {
            start: SliceIndex::FromEnd(NonZeroUsize::new(5).unwrap()),
            end: None,
            step: Step::forward(1),
        };
        let args = Args::parse_from(["slice", "-5:"]);
        assert_eq!(args.range, Some(tail.clone()));

        let args = Args::parse_from(["slice", "-l", "-5:", "text.txt"]);
        assert!(args.lines);
        assert_eq!(args.range, Some(tail.clone()));
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);

        let args = Args::parse_from(["slice", "-5:", "-l", "text.txt"]);
        assert!(args.lines);
        assert_eq!(args.range, Some(tail.clone()));
        assert_eq!(args.files, vec![PathBuf::from("text.txt")]);

        let args = Args::parse_from(["slice", "--explain", "-5:"]);
        assert!(args.explain);
        assert_eq!(args.range, Some(tail.clone()));
    }

    #[test]
    fn explain_flag_parses() {
        let args = Args::parse_from(["slice", "--explain", "10:20"]);
        assert!(args.explain);
        assert_eq!(
            args.range,
            Some(SliceRange {
                start: SliceIndex::FromStart(10),
                end: Some(SliceIndex::FromStart(20)),
                step: Step::forward(1),
            })
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
    fn max_record_size_parses_limit() {
        let args = Args::parse_from(["slice", "--max-record-size", "4KB", "-1:"]);
        assert_eq!(args.max_record_size(), Some(4_000));
    }

    #[test]
    fn max_record_size_unlimited_is_none() {
        let args = Args::parse_from(["slice", "--max-record-size", "unlimited", "-1:"]);
        assert_eq!(args.max_record_size(), None);
    }

    #[test]
    fn max_record_size_rejects_zero() {
        let err = Args::try_parse_from(["slice", "--max-record-size", "0", "-1:"])
            .expect_err("zero is invalid");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
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
    fn explain_and_translate_conflict() {
        let err = Args::try_parse_from(["slice", "--explain", "--translate=posix", "1:5"])
            .expect_err("--explain and --translate are mutually exclusive");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
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

    #[test]
    fn generate_parses_without_range() {
        let args = Args::parse_from(["slice", "--generate", "man"]);
        assert_eq!(args.generate, Some(Generate::Man));
        assert_eq!(args.range, None);
    }

    #[test]
    fn generate_value_enum_kebab_names() {
        for (value, expected) in [
            ("complete-bash", Generate::CompleteBash),
            ("complete-zsh", Generate::CompleteZsh),
            ("complete-fish", Generate::CompleteFish),
            ("complete-powershell", Generate::CompletePowershell),
            ("man", Generate::Man),
        ] {
            let args = Args::parse_from(["slice", "--generate", value]);
            assert_eq!(args.generate, Some(expected), "value {value}");
        }
        assert!(Args::try_parse_from(["slice", "--generate", "complete-power-shell"]).is_err());
    }

    #[test]
    fn generate_excludes_all_other_args() {
        assert!(Args::try_parse_from(["slice", "--generate", "complete-bash", "1:2"]).is_err());
        assert!(Args::try_parse_from(["slice", "--generate", "man", "-l"]).is_err());
        assert!(Args::try_parse_from(["slice", "--generate", "man", "--explain"]).is_err());
        assert!(Args::try_parse_from(["slice", "--generate", "man", "--translate=posix"]).is_err());
        assert!(Args::try_parse_from(["slice", "--generate", "man", "file.txt"]).is_err());
    }

    #[test]
    fn range_still_required_without_generate() {
        assert!(Args::try_parse_from(["slice"]).is_err());
        assert!(Args::try_parse_from(["slice", "-l"]).is_err());
    }

    #[test]
    fn translate_explicit_dialect_parses() {
        let args = Args::parse_from(["slice", "--translate=posix", ":5"]);
        assert_eq!(args.translate, Some(TranslateDialect::Posix));
        let args = Args::parse_from(["slice", "--translate=all", "1:5"]);
        assert_eq!(args.translate, Some(TranslateDialect::All));
    }

    #[test]
    fn translate_absent_is_none() {
        let args = Args::parse_from(["slice", ":5"]);
        assert_eq!(args.translate, None);
    }

    #[test]
    fn translate_bare_uses_platform_default() {
        let args = Args::parse_from(["slice", "--translate", ":5"]);
        let expected = if cfg!(all(target_os = "linux", target_env = "gnu")) {
            TranslateDialect::Gnu
        } else if cfg!(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )) {
            TranslateDialect::Bsd
        } else {
            TranslateDialect::Posix
        };
        assert_eq!(args.translate, Some(expected));
    }

    #[test]
    fn translate_require_equals_keeps_range_separate() {
        // A space-separated argument is not consumed as the dialect value, so
        // the range still reaches <RANGE> — including a hyphen-led tail range.
        let args = Args::parse_from(["slice", "--translate", "5:10"]);
        assert!(args.translate.is_some());
        assert_eq!(
            args.range,
            Some(SliceRange {
                start: SliceIndex::FromStart(5),
                end: Some(SliceIndex::FromStart(10)),
                step: Step::forward(1),
            })
        );
        let args = Args::parse_from(["slice", "--translate", "-5:"]);
        assert!(args.translate.is_some());
        assert_eq!(
            args.range,
            Some(SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(5).unwrap()),
                end: None,
                step: Step::forward(1),
            })
        );
    }

    #[test]
    fn translate_rejects_unknown_dialect() {
        assert!(Args::try_parse_from(["slice", "--translate=swahili", ":5"]).is_err());
    }
}
