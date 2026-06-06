#![no_main]

use libfuzzer_sys::fuzz_target;
use slice_command::{byte_mode, delimit_mode, line_mode};
use slice_command_fuzz::{reference, Mode, Plan};
use std::io::BufReader;

// The streaming implementation must produce exactly the bytes computed by the
// in-memory reference for every mode, range, and read-buffer size. Tiny
// capacities force records to span read boundaries; output must not depend on
// the capacity (the `--io-buffer-size` invariant).
fuzz_target!(|plan: Plan| {
    let range = plan.range();
    let expected = reference(&plan);
    for capacity in [1, 2, 3, 7, 8192] {
        let input = BufReader::with_capacity(capacity, plan.data.as_slice());
        let mut out = Vec::new();
        match &plan.mode {
            Mode::Line => line_mode(input, &mut out, &range),
            Mode::Byte => byte_mode(input, &mut out, &range),
            Mode::Delimit { delimiter } => delimit_mode(input, &mut out, delimiter, &range),
        }
        .expect("in-memory I/O cannot fail");
        assert_eq!(
            out, expected,
            "streaming output diverged from the reference at capacity {capacity}: {plan:?}"
        );
    }
});
