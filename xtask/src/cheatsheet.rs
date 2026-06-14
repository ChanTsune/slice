//! The cheatsheet model: parse `docs/cheatsheet.toml` and expose the
//! repository paths and shared rendering helpers used by `gen` and `check`.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const README_START: &str = "<!-- CHEATSHEET:START -->";
pub const README_END: &str = "<!-- CHEATSHEET:END -->";

#[derive(Deserialize)]
pub struct Cheatsheet {
    pub rows: Vec<Row>,
    #[serde(default)]
    pub non_mappings: Vec<NonMapping>,
    #[serde(default)]
    pub caveats: Caveats,
}

#[derive(Deserialize)]
pub struct Row {
    pub id: String,
    pub group: Group,
    pub task: String,
    pub tools: String,
    pub slice: String,
    pub verify: Verify,
    #[serde(default)]
    pub sample_input: String,
    /// Expected slice output for a `slice-only` row, decoded like `sample_input`.
    /// When present, `check` asserts the slice output matches it exactly;
    /// otherwise it only asserts the output is non-empty.
    #[serde(default)]
    pub expect: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Explicit command `check` runs for parity, overriding the `tools` display
    /// string. Needed when `tools` shows a GNU-only spelling for documentation
    /// but the portable equivalent is what should be verified under `posix`.
    #[serde(default)]
    pub run: Option<String>,
}

#[derive(Deserialize)]
pub struct NonMapping {
    pub label: String,
    pub cmd: String,
    pub why_not: String,
}

#[derive(Deserialize, Default)]
pub struct Caveats {
    #[serde(default)]
    pub items: Vec<String>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Line,
    Byte,
    Stepped,
    Special,
}

impl Group {
    /// Section title used as a query-matched H2 in the HTML and as a heading in
    /// the README table.
    pub fn title(self) -> &'static str {
        match self {
            Group::Line => "Print a range of lines (head, tail, sed, awk)",
            Group::Byte => "Byte ranges from a file (head -c, tail -c, dd without dd)",
            Group::Stepped => "Every Nth line (sed/awk only — slice does it too)",
            Group::Special => "NUL-delimited records and other special cases",
        }
    }

    pub fn anchor(self) -> &'static str {
        match self {
            Group::Line => "lines",
            Group::Byte => "bytes",
            Group::Stepped => "stepped",
            Group::Special => "special",
        }
    }

    /// Plain-text section label for the llms.txt cheatsheet.
    pub fn llms_label(self) -> &'static str {
        match self {
            Group::Line => "Lines:",
            Group::Byte => "Bytes:",
            Group::Stepped => "Every Nth line (head/tail cannot express this):",
            Group::Special => "Special:",
        }
    }

    /// Display order of the sections.
    pub const ORDER: [Group; 4] = [Group::Line, Group::Byte, Group::Stepped, Group::Special];

    /// Sections the README shows inline. The README is the showcase, not the
    /// full reference: it keeps the common line ranges and defers byte ranges,
    /// every-Nth-line, and NUL records to the docs site.
    pub const README_ORDER: [Group; 1] = [Group::Line];
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verify {
    Posix,
    Gnu,
    #[serde(rename = "slice-only")]
    SliceOnly,
}

impl Row {
    /// The `slice` command split into its argument vector (binary name dropped).
    pub fn slice_args(&self) -> Vec<String> {
        self.slice
            .split_whitespace()
            .skip(1)
            .map(str::to_owned)
            .collect()
    }

    /// Decode the sample input's `\n` `\t` `\r` `\0` `\\` escapes into raw bytes.
    pub fn sample_bytes(&self) -> Vec<u8> {
        decode_escapes(&self.sample_input)
    }

    /// Decode the expected output's escapes, when the row declares one.
    pub fn expect_bytes(&self) -> Option<Vec<u8>> {
        self.expect.as_deref().map(decode_escapes)
    }
}

fn decode_escapes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let (push, advance) = match bytes[i + 1] {
                b'n' => (b'\n', 2),
                b't' => (b'\t', 2),
                b'r' => (b'\r', 2),
                b'0' => (0, 2),
                b'\\' => (b'\\', 2),
                _ => (bytes[i], 1),
            };
            out.push(push);
            i += advance;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

/// Repository root, located relative to this crate's manifest.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level below the repo root")
        .to_path_buf()
}

pub fn cheatsheet_path() -> PathBuf {
    repo_root().join("docs/cheatsheet.toml")
}

pub fn readme_path() -> PathBuf {
    repo_root().join("README.md")
}

pub fn html_output_path() -> PathBuf {
    repo_root().join("docs/index.html")
}

pub fn sitemap_output_path() -> PathBuf {
    repo_root().join("docs/sitemap.xml")
}

pub fn og_svg_path() -> PathBuf {
    repo_root().join("docs/og.svg")
}

pub fn llms_output_path() -> PathBuf {
    repo_root().join("docs/llms.txt")
}

pub fn template_path() -> PathBuf {
    repo_root().join("xtask/templates/cheatsheet.html")
}

pub fn llms_template_path() -> PathBuf {
    repo_root().join("xtask/templates/llms.txt")
}

/// Canonical site URL, shared by the HTML/JSON-LD, the sitemap, and llms.txt.
pub const SITE_URL: &str = "https://chantsune.github.io/slice/";

pub fn load() -> Result<Cheatsheet, String> {
    let path = cheatsheet_path();
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
}
