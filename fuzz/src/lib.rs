//! Shared input shapes and the in-memory reference implementation used by the
//! fuzz targets.

use arbitrary::Arbitrary;
use slice_command::range::SliceRange;
use std::num::NonZeroUsize;

#[derive(Arbitrary, Debug)]
pub enum Mode {
    Line,
    Byte,
    // An empty delimiter is reachable from the CLI (`--delimiter ""`) and is
    // defined to split into single bytes, so it is deliberately not filtered.
    Delimit { delimiter: Vec<u8> },
}

#[derive(Arbitrary, Debug)]
pub struct Plan {
    pub mode: Mode,
    pub start: usize,
    pub end: usize,
    pub step: Option<NonZeroUsize>,
    pub data: Vec<u8>,
}

impl Plan {
    pub fn range(&self) -> SliceRange {
        SliceRange {
            start: self.start,
            end: self.end,
            step: self.step,
        }
    }
}

/// Compute the expected output entirely in memory: split `data` into records,
/// then select `[start, end)` with `step` using explicit index arithmetic.
///
/// The selection is intentionally written differently from the streaming
/// implementation's `take/skip/step_by` chain so that a bug there cannot
/// cancel out in the comparison.
pub fn reference(plan: &Plan) -> Vec<u8> {
    let records = match &plan.mode {
        Mode::Line => split_after(&plan.data, b"\n"),
        Mode::Byte => single_bytes(&plan.data),
        Mode::Delimit { delimiter } if delimiter.is_empty() => single_bytes(&plan.data),
        Mode::Delimit { delimiter } => split_after(&plan.data, delimiter),
    };

    let step = plan.step.map_or(1, NonZeroUsize::get);
    let end = plan.end.min(records.len());
    let mut out = Vec::new();
    let mut i = plan.start;
    while i < end {
        out.extend_from_slice(&records[i]);
        // `step` itself can be huge, so saturate instead of overflowing.
        i = i.saturating_add(step);
    }
    out
}

fn single_bytes(data: &[u8]) -> Vec<Vec<u8>> {
    data.iter().map(|b| vec![*b]).collect()
}

/// Split after each leftmost non-overlapping occurrence of `delimiter`; each
/// record keeps its trailing delimiter and an unterminated tail is kept as-is.
fn split_after(data: &[u8], delimiter: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + delimiter.len() <= data.len() {
        if &data[i..i + delimiter.len()] == delimiter {
            i += delimiter.len();
            records.push(data[start..i].to_vec());
            start = i;
        } else {
            i += 1;
        }
    }
    if start < data.len() {
        records.push(data[start..].to_vec());
    }
    records
}
