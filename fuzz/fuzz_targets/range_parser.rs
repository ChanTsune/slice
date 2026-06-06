#![no_main]

use libfuzzer_sys::fuzz_target;
use slice_command::range::SliceRange;

// The range parser must accept or reject any string without panicking.
fuzz_target!(|s: &str| {
    let _ = s.parse::<SliceRange>();
});
