#![doc = include_str!("../README.md")]

pub mod cli;
mod ext;
pub mod range;
mod run;

pub use run::entry;
