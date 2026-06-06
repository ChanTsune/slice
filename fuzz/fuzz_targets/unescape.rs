#![no_main]

use libfuzzer_sys::fuzz_target;
use slice_command::cli::unescape;

// Unescaping must never panic, and every escape sequence shrinks (or keeps)
// the byte length, so the output can never outgrow the input.
fuzz_target!(|s: &str| {
    if let Ok(out) = unescape(s) {
        assert!(out.len() <= s.len());
    }
});
