#![doc = include_str!("../README.md")]

pub mod cli;
mod ext;
pub mod range;
mod run;

pub use run::{byte_mode, delimit_mode, entry, line_mode};
