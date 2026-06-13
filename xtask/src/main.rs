//! Cheatsheet generator and verifier.
//!
//! `cargo xtask gen` renders `docs/cheatsheet.toml` into the README table and
//! `docs/index.html`. `cargo xtask check` proves the generated README is fresh
//! and that every row's `slice` command matches its coreutils/sed/awk/dd recipe
//! byte-for-byte on the current machine.

mod cheatsheet;
mod check;
mod gen;

use std::process::ExitCode;

const HELP: &str = "\
xtask — slice repository tasks

Usage:
  cargo xtask gen                 Regenerate the README table and docs/index.html
  cargo xtask check [--slice P]   Verify README freshness and per-row parity
                                  (--slice picks the slice binary;
                                   default ./target/release/slice)
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("gen") => run(gen::run()),
        Some("check") => run(check::run(args.collect())),
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        other => {
            if let Some(cmd) = other {
                eprintln!("unknown subcommand: {cmd}\n");
            }
            eprint!("{HELP}");
            ExitCode::FAILURE
        }
    }
}

fn run(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
