#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use slice_command::range::SliceRange;
use slice_command::{byte_mode, delimit_mode, line_mode};
use std::num::NonZeroUsize;

#[derive(Arbitrary, Debug)]
enum Mode {
    Line,
    Byte,
    // An empty delimiter is reachable from the CLI (`--delimiter ""`) and is
    // defined to split into single bytes, so it is deliberately not filtered.
    Delimit { delimiter: Vec<u8> },
}

#[derive(Arbitrary, Debug)]
struct Plan {
    mode: Mode,
    start: usize,
    end: usize,
    step: Option<NonZeroUsize>,
    data: Vec<u8>,
}

// Every mode must handle any binary input and any range without panicking,
// and only ever emits bytes it has read, so the output cannot outgrow the
// input. `explain` must likewise render any range without panicking.
fuzz_target!(|plan: Plan| {
    let range = SliceRange {
        start: plan.start,
        end: plan.end,
        step: plan.step,
    };
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
