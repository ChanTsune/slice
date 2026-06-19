//! `cargo xtask check` — prove the README is freshly generated and that every
//! cheatsheet row's `slice` command matches its coreutils/sed/awk/dd recipe on
//! this machine. Missing or non-GNU tools are skipped, never failed, so CI on a
//! BSD/Windows runner stays green.

use crate::cheatsheet::{self, Row, Verify};
use crate::gen;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub fn run(args: Vec<String>) -> Result<(), String> {
    let slice_bin = parse_slice_arg(&args)?;
    let sheet = cheatsheet::load()?;

    check_readme_fresh(&sheet)?;
    check_gen_idempotent()?;
    check_og_svg(&sheet)?;

    if !slice_bin.exists() {
        return Err(format!(
            "slice binary not found at {}; run `cargo build --release` or pass --slice <path>",
            slice_bin.display()
        ));
    }

    let mut pass = 0usize;
    let mut skip = 0usize;
    for row in &sheet.rows {
        match verify_row(row, &slice_bin)? {
            Outcome::Pass => {
                pass += 1;
                println!("ok    {}", row.id);
            }
            Outcome::Skip(reason) => {
                skip += 1;
                println!("skip  {} ({reason})", row.id);
            }
        }
    }
    println!(
        "\nparity: {pass} passed, {skip} skipped, {} rows",
        sheet.rows.len()
    );

    check_translate_parity(&slice_bin)?;
    Ok(())
}

fn parse_slice_arg(args: &[String]) -> Result<PathBuf, String> {
    let default = || cheatsheet::repo_root().join("target/release/slice");
    match args.split_first() {
        None => Ok(default()),
        Some((flag, rest)) if flag == "--slice" => match rest {
            [path] => Ok(PathBuf::from(path)),
            [] => Err("--slice needs a path".to_owned()),
            _ => Err("too many arguments after --slice".to_owned()),
        },
        Some((flag, [])) if flag.starts_with("--slice=") => {
            Ok(PathBuf::from(&flag["--slice=".len()..]))
        }
        Some((other, _)) => Err(format!("unknown argument: {other}")),
    }
}

/// The README must already contain exactly what `gen` would write, so the
/// committed table never drifts from the SSOT.
fn check_readme_fresh(sheet: &cheatsheet::Cheatsheet) -> Result<(), String> {
    let path = cheatsheet::readme_path();
    let readme =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let regenerated = gen::splice_readme(&readme, &gen::render_readme_table(sheet))?;
    if regenerated != readme {
        return Err(format!(
            "{} is stale; run `cargo xtask gen` and commit the result",
            path.display()
        ));
    }
    Ok(())
}

/// `gen` derives the `docs/` artifacts deterministically, so two runs must
/// agree. The artifacts themselves are build outputs (gitignored), so freshness
/// is "idempotent", not "matches a committed file".
fn check_gen_idempotent() -> Result<(), String> {
    let artifacts = [
        ("docs/index.html", cheatsheet::html_output_path()),
        ("docs/sitemap.xml", cheatsheet::sitemap_output_path()),
        ("docs/llms.txt", cheatsheet::llms_output_path()),
    ];
    gen::run()?;
    let first: Vec<Vec<u8>> = artifacts
        .iter()
        .map(|(_, path)| std::fs::read(path).unwrap_or_default())
        .collect();
    gen::run()?;
    for ((name, path), before) in artifacts.iter().zip(first) {
        let after = std::fs::read(path).unwrap_or_default();
        if before != after {
            return Err(format!("cargo xtask gen is not idempotent for {name}"));
        }
    }
    Ok(())
}

/// The Open Graph image hand-draws a few cheatsheet rows. It cannot be
/// generated (it is hand-illustrated), so instead assert every `slice ...`
/// command chip it renders still matches some row's `slice` value. A rename in
/// the SSOT that left the image behind would fail here.
fn check_og_svg(sheet: &cheatsheet::Cheatsheet) -> Result<(), String> {
    let path = cheatsheet::og_svg_path();
    let svg =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let valid: std::collections::HashSet<&str> =
        sheet.rows.iter().map(|r| r.slice.as_str()).collect();
    for chip in svg_text_commands(&svg) {
        if !valid.contains(chip.as_str()) {
            return Err(format!(
                "{}: renders `{chip}`, which matches no cheatsheet row's slice value",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Extract the body of each `<text>` element that renders a `slice <args>`
/// command. Scanning `<text>` bodies (not raw `slice` substrings) skips XML
/// comments and the bare `slice` wordmark.
fn svg_text_commands(svg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(tag) = rest.find("<text") {
        rest = &rest[tag..];
        let Some(open_end) = rest.find('>') else {
            break;
        };
        let after_open = &rest[open_end + 1..];
        let Some(close) = after_open.find("</text>") else {
            break;
        };
        let body = after_open[..close].trim();
        if let Some(args) = body.strip_prefix("slice ") {
            if !args.trim().is_empty() {
                out.push(body.to_owned());
            }
        }
        rest = &after_open[close + "</text>".len()..];
    }
    out
}

enum Outcome {
    Pass,
    Skip(String),
}

fn verify_row(row: &Row, slice_bin: &PathBuf) -> Result<Outcome, String> {
    let input = row.sample_bytes();
    let slice_out = run_slice(slice_bin, &row.slice_args(), &input)?;

    match row.verify {
        Verify::SliceOnly => match row.expect_bytes() {
            Some(expected) if slice_out == expected => Ok(Outcome::Pass),
            Some(expected) => Err(format!(
                "{}: slice output differs from expected\n  slice:    {:?}\n  expected: {:?}",
                row.id, slice_out, expected
            )),
            None if slice_out.is_empty() => Err(format!("{}: slice produced no output", row.id)),
            None => Ok(Outcome::Pass),
        },
        Verify::Posix | Verify::Gnu => {
            let needs_gnu = row.verify == Verify::Gnu;
            let tools = row.run.as_deref().unwrap_or(&row.tools);
            let Some(cmd) = resolve_tool(tools, needs_gnu)? else {
                return Ok(Outcome::Skip(if needs_gnu {
                    "no GNU coreutils".to_owned()
                } else {
                    "tool unavailable".to_owned()
                }));
            };
            let tool_out = run_shell(&cmd, &input)?;
            if tool_out == slice_out {
                Ok(Outcome::Pass)
            } else {
                Err(format!(
                    "{}: slice output differs from `{cmd}`\n  slice: {:?}\n  tool:  {:?}",
                    row.id, slice_out, tool_out
                ))
            }
        }
    }
}

/// Pick the runnable spelling from a `tools` field. The field may list several
/// spellings separated by `  /  `; the first one whose program exists (and is
/// GNU, when required) wins. GNU detection prefers the `g`-prefixed Homebrew
/// names, then falls back to a `--version` probe of the bare program.
fn resolve_tool(tools: &str, needs_gnu: bool) -> Result<Option<String>, String> {
    for spelling in tools.split("  /  ").map(str::trim) {
        let Some(program) = spelling.split_whitespace().next() else {
            continue;
        };
        if needs_gnu {
            if let Some(gnu) = gnu_program(program) {
                // Rewrite only the leading program token to the GNU binary.
                let rest = spelling[program.len()..].to_owned();
                return Ok(Some(format!("{gnu}{rest}")));
            }
        } else if which(program).is_some() {
            return Ok(Some(spelling.to_owned()));
        }
    }
    Ok(None)
}

/// Return a runnable GNU spelling of `program`, or `None` if no GNU build is
/// reachable. Prefers `g<program>` (Homebrew coreutils), else accepts the bare
/// program when its `--version` advertises GNU.
fn gnu_program(program: &str) -> Option<String> {
    let g = format!("g{program}");
    if which(&g).is_some() {
        return Some(g);
    }
    if which(program).is_some() && is_gnu(program) {
        return Some(program.to_owned());
    }
    None
}

fn is_gnu(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("GNU coreutils"))
        .unwrap_or(false)
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(program);
        candidate.is_file().then_some(candidate)
    })
}

fn run_slice(bin: &PathBuf, args: &[String], input: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawning {}: {e}", bin.display()))?;
    feed(&mut child, input)?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("waiting on slice: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "slice {:?} exited with {}: {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.stdout)
}

fn run_shell(cmd: &str, input: &[u8]) -> Result<Vec<u8>, String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawning `{cmd}`: {e}"))?;
    feed(&mut child, input)?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("waiting on `{cmd}`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`{cmd}` exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(out.stdout)
}

fn feed(child: &mut std::process::Child, input: &[u8]) -> Result<(), String> {
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(input)
        .map_err(|e| format!("writing stdin: {e}"))
}

/// The mode a translate case runs in. The flags feed both `--translate` (which
/// command is generated) and the `slice` run that command is checked against.
#[derive(Clone, Copy)]
enum Mode {
    Lines,
    Bytes,
    /// Empty `--delimiter`: slices bytes, but through the delimiter flag path
    /// rather than `-b` — the case that must classify as byte mode, not custom.
    BytesEmptyDelim,
    /// A real custom delimiter: no standard tool selects by it, so translation
    /// must report no equivalent and there is no command to run.
    Custom,
}

impl Mode {
    fn flags(self) -> Vec<String> {
        match self {
            Mode::Lines => vec![],
            Mode::Bytes => vec!["-b".to_owned()],
            Mode::BytesEmptyDelim => vec!["--delimiter".to_owned(), String::new()],
            Mode::Custom => vec!["--delimiter".to_owned(), ",".to_owned()],
        }
    }
    fn byte_oracle(self) -> bool {
        matches!(self, Mode::Bytes | Mode::BytesEmptyDelim)
    }
    fn label(self) -> &'static str {
        match self {
            Mode::Lines => "lines",
            Mode::Bytes => "bytes",
            Mode::BytesEmptyDelim => "empty-delim",
            Mode::Custom => "custom-delim",
        }
    }
}

/// What translation a case must produce, so a generator regression in either
/// direction is caught: a translatable range silently turning into "no
/// equivalent" (some dialect must still emit a command), and an untranslatable
/// range wrongly emitting one (no dialect may).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// At least one dialect must emit a runnable command (verified for real
    /// where the tool exists).
    Runnable,
    /// Every dialect must report no equivalent (custom delimiter, genuine
    /// strided byte selection).
    Untranslatable,
}

/// Ranges crossed with every dialect, covering each translate arm: head/tail/
/// window (end inside and past the input)/single/drop-last (`sed '$d'`, `head
/// -n/-c -N`)/stepped lines (with a load-bearing bound), byte head/window/tail/
/// skip/single, the degenerate single-byte step, the `+`/`+-` relative-end
/// desugars, the empty-delimiter byte path, genuine strides and custom
/// delimiters (untranslatable), copy, and the empty range.
const TRANSLATE_CASES: &[(&str, Mode, Expect)] = &[
    (":", Mode::Lines, Expect::Runnable),
    (":5", Mode::Lines, Expect::Runnable),
    ("1:5", Mode::Lines, Expect::Runnable),
    // End inside the 10-line input and selection short of the last line, so an
    // off-by-one in the generated `sed` end bound changes the output.
    ("3:8", Mode::Lines, Expect::Runnable),
    ("6:7", Mode::Lines, Expect::Runnable),
    ("9:", Mode::Lines, Expect::Runnable),
    ("-5:", Mode::Lines, Expect::Runnable),
    // Single-line drop-last: `sed '$d'` (posix) and `head -n -1` (gnu).
    (":-1", Mode::Lines, Expect::Runnable),
    (":-3", Mode::Lines, Expect::Runnable),
    ("::2", Mode::Lines, Expect::Runnable),
    ("1::2", Mode::Lines, Expect::Runnable),
    // Bounded stride whose inclusive end IS the last selected row, so the awk
    // upper bound is load-bearing (`NR<=6` vs `NR<=5` diverges).
    ("1:6:2", Mode::Lines, Expect::Runnable),
    ("1:7:2", Mode::Lines, Expect::Runnable),
    ("5:+10", Mode::Lines, Expect::Runnable),
    // `+-` window desugar: 5:+-2 -> [3,7) -> `sed -n '4,7p'`, end inside input.
    ("5:+-2", Mode::Lines, Expect::Runnable),
    // Empty range: GNU emits the zero-count head (`head -n 0` for lines,
    // `head -c 0` for bytes; selects nothing); other dialects report no
    // equivalent and are skipped, like the drop-last forms. A custom delimiter
    // has no equivalent even when empty.
    ("5:3", Mode::Lines, Expect::Runnable),
    (":5", Mode::Bytes, Expect::Runnable),
    ("5:15", Mode::Bytes, Expect::Runnable),
    ("5:", Mode::Bytes, Expect::Runnable),
    ("-5:", Mode::Bytes, Expect::Runnable),
    // Single-byte drop-last: `head -c -1` (gnu).
    (":-1", Mode::Bytes, Expect::Runnable),
    (":-3", Mode::Bytes, Expect::Runnable),
    // Plain single-byte window from an offset: `dd bs=1 skip=6 count=1`.
    ("6:7", Mode::Bytes, Expect::Runnable),
    ("5:6:2", Mode::Bytes, Expect::Runnable),
    ("5:7:2", Mode::Bytes, Expect::Runnable),
    (":1:2", Mode::Bytes, Expect::Runnable),
    // `+`/`+-` byte window desugars.
    ("5:+10", Mode::Bytes, Expect::Runnable),
    ("5:+-2", Mode::Bytes, Expect::Runnable),
    ("5:3", Mode::Bytes, Expect::Runnable),
    ("5:8:2", Mode::Bytes, Expect::Untranslatable),
    ("::2", Mode::Bytes, Expect::Untranslatable),
    (":5", Mode::BytesEmptyDelim, Expect::Runnable),
    ("5:6:2", Mode::BytesEmptyDelim, Expect::Runnable),
    ("5:15", Mode::BytesEmptyDelim, Expect::Runnable),
    (":5", Mode::Custom, Expect::Untranslatable),
    ("::2", Mode::Custom, Expect::Untranslatable),
    ("5:3", Mode::Custom, Expect::Untranslatable),
];

/// Ranges that select nothing (start past end). The parity loop above exercises
/// their `--translate` form where the GNU zero-count head runs (`head -n 0` for
/// lines, `head -c 0` for bytes), but that leaves the one invariant that
/// actually matters — slice produces no output — unverified on a non-GNU box
/// (every other dialect is "no equivalent" and skipped). These are asserted
/// separately, independent of any dialect or external tool.
const EMPTY_RANGES: &[(&str, Mode)] = &[("5:3", Mode::Lines), ("5:3", Mode::Bytes)];

// Oracle inputs. `TEXT_TERM` (also the empty-range probe — long enough to
// expose a non-empty selection) is fully newline-terminated; `TEXT_NOTERM`
// drops the final newline; `BINARY` carries NUL and high bytes with no
// trailing newline.
const TEXT_TERM: &[u8] = b"l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\n";
const TEXT_NOTERM: &[u8] = b"l0\nl1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9";
const BINARY: &[u8] = &[
    0x00, 0x01, 0x80, 0xff, 0x0a, 0x41, 0x42, 0x00, 0xfe, 0x7f, 0x10, 0x20, 0x99, 0xaa, 0xbb, 0xcc,
    0xdd, 0x05, 0x80, 0x01,
];

/// Prove each `--translate` command actually reproduces `slice` byte-for-byte by
/// running both on the same input. Portable forms (POSIX, `head -c`, awk on
/// text) are checked everywhere a shell exists; GNU-only forms run on GNU boxes
/// and are skipped on BSD by design. Tier floors below reject a vacuous pass
/// where missing tools silently skipped everything.
fn check_translate_parity(slice_bin: &PathBuf) -> Result<(), String> {
    // The command is run verbatim through `sh`; with no POSIX shell (Windows)
    // there is nothing to verify. CI skips this xtask on Windows, so a missing
    // shell here is unexpected but handled rather than spuriously failing.
    if run_shell("true", b"").is_err() {
        println!("\ntranslate parity: skipped (no POSIX shell)");
        return Ok(());
    }
    let gnu_env = is_gnu("head");
    let awk_ok = which("awk").is_some();
    let (mut posix, mut bsd, mut gnu, mut awk, mut skip) = (0usize, 0usize, 0usize, 0usize, 0usize);

    for &(range, mode, expect) in TRANSLATE_CASES {
        let flags = mode.flags();
        // The oracle output depends only on (flags, range, input), never on the
        // dialect, so compute it once per input for the whole case rather than
        // re-spawning slice inside the dialect loop.
        let oracles: Vec<(&str, &[u8], Vec<u8>)> = if expect == Expect::Runnable {
            case_inputs(mode.byte_oracle())
                .iter()
                .map(|&(label, input)| {
                    oracle_run(slice_bin, &flags, range, input).map(|out| (label, input, out))
                })
                .collect::<Result<_, _>>()?
        } else {
            Vec::new()
        };

        let mut emitted_any = false;
        for dialect in ["posix", "bsd", "gnu", "awk"] {
            let mut targs = flags.clone();
            targs.push(format!("--translate={dialect}"));
            targs.push(range.to_owned());
            let out = run_slice(slice_bin, &targs, b"")?;
            let text = String::from_utf8_lossy(&out);
            // Single-dialect output is a `# <label>` comment line followed by the
            // command on the next line; an untranslatable range is the comment
            // line alone. The command is therefore the first non-comment line.
            let lines: Vec<&str> = text.lines().collect();
            let cmd = lines
                .iter()
                .find(|l| !l.starts_with('#'))
                .copied()
                .unwrap_or("")
                .to_owned();

            // No command line means slice reported no equivalent for this
            // dialect (untranslatable); skip it.
            if cmd.is_empty() {
                skip += 1;
                continue;
            }

            // A translatable result is exactly a `# <label>` line then the
            // command line; assert that shape so a future format drift fails
            // loudly here instead of silently picking the wrong line as the
            // command.
            if lines.len() != 2 || !lines[0].starts_with("# ") || lines[1].starts_with('#') {
                return Err(format!(
                    "[{} {range} {dialect}] unexpected translate output shape: {text:?}",
                    mode.label()
                ));
            }

            if expect != Expect::Runnable {
                return Err(format!(
                    "[{} {range} {dialect}] expected no command but translate emitted `{cmd}`",
                    mode.label()
                ));
            }
            emitted_any = true;

            // The realized tier can differ from the requested dialect (a posix
            // request for a byte head becomes `head -c`, tier bsd) and awk can
            // surface under any dialect (posix `::2` -> awk), so derive the tier
            // from the command's shape rather than the requested dialect.
            let is_awk = cmd.starts_with("awk");
            let bucket = if is_awk { "awk" } else { classify_tier(&cmd) };
            for &(label, input, ref oracle) in &oracles {
                // awk is byte-exact only on terminated, NUL-free text; it never
                // reaches byte mode (AWK_BYTE_REASON), so this only drops the
                // unterminated line shape.
                if is_awk && label != "text-term" {
                    continue;
                }
                let got = match run_shell(&cmd, input) {
                    Ok(got) => got,
                    // A GNU-only spelling errors on BSD by design; an awk form
                    // needs awk present. Any other portable command that fails
                    // to run is a real defect.
                    Err(_) if bucket == "gnu" || (is_awk && !awk_ok) => {
                        skip += 1;
                        continue;
                    }
                    Err(e) => {
                        return Err(format!(
                            "translate command failed to run [{} {range} {dialect} / {label}]: {e}",
                            mode.label()
                        ))
                    }
                };
                if &got != oracle {
                    return Err(format!(
                        "translate parity mismatch [{} {range} {dialect} / {label}]\n  cmd:   {cmd}\n  slice: {oracle:?}\n  got:   {got:?}",
                        mode.label()
                    ));
                }
                match bucket {
                    "awk" => awk += 1,
                    "bsd" => bsd += 1,
                    "gnu" => gnu += 1,
                    _ => posix += 1,
                }
            }
        }
        // A translatable range that emitted no command on any dialect has
        // regressed to a false "no equivalent".
        if expect == Expect::Runnable && !emitted_any {
            return Err(format!(
                "[{} {range}] expected a translatable command but every dialect reported no equivalent",
                mode.label()
            ));
        }
    }

    // The empty range's defining property is that it selects nothing; assert it
    // directly so it holds on every platform, not only where `head -n 0` runs.
    for &(range, mode) in EMPTY_RANGES {
        let out = oracle_run(slice_bin, &mode.flags(), range, TEXT_TERM)?;
        if !out.is_empty() {
            return Err(format!(
                "[{} {range}] empty range must select nothing but slice produced {out:?}",
                mode.label()
            ));
        }
    }

    println!("\ntranslate parity: posix={posix} bsd={bsd} gnu={gnu} awk={awk}, {skip} skipped");

    // Reject a vacuous pass: portable forms must actually have run and matched.
    // GNU forms are required only where GNU coreutils exist (Linux CI); a
    // BSD-only runner legitimately skips them.
    if posix < 6 {
        return Err(format!(
            "translate parity ran too few POSIX checks ({posix}); the environment may lack tools"
        ));
    }
    if awk_ok && awk < 3 {
        return Err(format!("translate parity ran too few awk checks ({awk})"));
    }
    if bsd < 1 {
        return Err(format!(
            "translate parity ran no `head -c` (bsd) checks ({bsd})"
        ));
    }
    if gnu_env && gnu < 3 {
        return Err(format!(
            "translate parity ran too few GNU checks ({gnu}) on a GNU coreutils box"
        ));
    }
    Ok(())
}

fn oracle_run(
    bin: &PathBuf,
    flags: &[String],
    range: &str,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let mut args = flags.to_vec();
    args.push(range.to_owned());
    run_slice(bin, &args, input)
}

/// The realized portability tier of a translate command, derived from its shape.
/// This must track every GNU-only spelling `src/range.rs` emits: the drop-last
/// forms `head -n -N`/`head -c -N` (`translate_lag`), the `sed -n 'F~Sp'` stride
/// (`translate_unbounded`), and the zero-count empty range `head -n 0`/`head -c 0`
/// (`empty_candidate`) — POSIX/BSD `head` reject a count of 0. `head -c N` is
/// BSD+; everything else (`sed -n` ranges, `sed '$d'`, `head -n N`, `tail`, `dd`,
/// `cat`) is POSIX.
fn classify_tier(cmd: &str) -> &'static str {
    if cmd.starts_with("head -n -")
        || cmd.starts_with("head -c -")
        || cmd == "head -n 0"
        || cmd == "head -c 0"
        || cmd.contains('~')
    {
        "gnu"
    } else if cmd.starts_with("head -c ") {
        "bsd"
    } else {
        "posix"
    }
}

/// Inputs the oracle compares against, by mode: byte tools on binary, line
/// tools on both a terminated and an unterminated shape. awk commands restrict
/// themselves to the terminated shape at the call site (it re-terminates an
/// unterminated final line and is NUL-lossy).
fn case_inputs(byte_mode: bool) -> &'static [(&'static str, &'static [u8])] {
    if byte_mode {
        &[("binary", BINARY)]
    } else {
        &[("text-term", TEXT_TERM), ("text-noterm", TEXT_NOTERM)]
    }
}
