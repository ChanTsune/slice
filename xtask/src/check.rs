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
