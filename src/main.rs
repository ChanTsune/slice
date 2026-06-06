use clap::{CommandFactory, Parser};
use slice_command::{cli, entry};
use std::process::ExitCode;

fn main() -> ExitCode {
    match entry(cli::Args::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        // Usage errors keep clap's formatting and exit code by being rendered
        // here, at the binary boundary; the library itself never exits.
        Err(e) => cli::Args::command()
            .error(clap::error::ErrorKind::ValueValidation, e)
            .exit(),
    }
}
