#![no_main]

use libfuzzer_sys::fuzz_target;
use slice_command::{byte_mode, delimit_mode, line_mode};
use slice_command_fuzz::{Mode, Plan};

// Every mode must handle any binary input and any range without panicking,
// and only ever emits bytes it has read, so the output cannot outgrow the
// input. `explain` must likewise render any range without panicking.
fuzz_target!(|plan: Plan| {
    let range = plan.range();
    let _ = range.explain("line");

    let mut out = Vec::new();
    let result = match &plan.mode {
        Mode::Line => line_mode(plan.data.as_slice(), &mut out, &range),
        Mode::Byte => byte_mode(plan.data.as_slice(), &mut out, &range),
        Mode::Delimit { delimiter } => {
            delimit_mode(plan.data.as_slice(), &mut out, delimiter, &range)
        }
    };
    result.expect("in-memory I/O cannot fail");
    assert!(out.len() <= plan.data.len());
});
