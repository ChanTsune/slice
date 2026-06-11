use std::{
    num::{IntErrorKind, NonZeroUsize, ParseIntError},
    str::FromStr,
};

/// One bound of a slice. `FromEnd` is a distance back from the end of input;
/// the lexeme `-0` normalizes to `FromStart(0)` at parse time (Python has no
/// -0), so zero distance is unrepresentable here.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) enum SliceIndex {
    FromStart(usize),
    FromEnd(NonZeroUsize),
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SliceRange {
    pub(crate) start: usize,
    /// `None` means unbounded (run to the end of input).
    pub(crate) end: Option<SliceIndex>,
    pub(crate) step: Option<NonZeroUsize>,
}

/// Which numeric field of a `start:end:step` range failed to parse.
#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) enum RangeField {
    Start,
    End,
    Step,
}

impl std::fmt::Display for RangeField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RangeField::Start => "start",
            RangeField::End => "end",
            RangeField::Step => "step",
        })
    }
}

/// Error produced while parsing a `SliceRange` from its `start:end:step` form.
#[derive(Clone, Eq, PartialEq, Debug, thiserror::Error)]
pub(crate) enum ParseSliceRangeError {
    #[error("range requires a ':' separator (e.g. '3:4', '3:', or ':3')")]
    MissingColon,
    #[error("invalid {field} value '{value}': {source}")]
    InvalidField {
        field: RangeField,
        value: String,
        source: ParseIntError,
    },
    #[error("a relative end ('+' or '+-') requires a count (e.g. '5:+3' or '5:+-3')")]
    MissingRelativeAmount,
    #[error("too many ':' separators in range (expected at most start:end:step)")]
    TooManyParts,
}

/// How a parsed range executes. `SliceRange` stays the `start:end:step` as
/// written; classifying it once lets dispatch match on the processing shape
/// instead of re-testing field combinations at every branch.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum SlicePlan {
    /// Bounded with `start >= end` (`5:3`, `:0`): selects nothing in any mode,
    /// so the input need not be read at all.
    Empty,
    /// Selects the whole input unchanged (`:`, `::`, `0::1`): the output equals
    /// the input byte-for-byte in every mode, so splitting can be skipped in
    /// favor of a verbatim copy.
    Copy,
    /// Unit step with an offset or bound: the selected elements are contiguous,
    /// so the output is the single span `[start, end)`.
    Window { start: usize, end: Option<usize> },
    /// `step > 1` selects non-contiguous elements; only the element-by-element
    /// pipeline can express it.
    Stepped {
        start: usize,
        end: Option<usize>,
        step: NonZeroUsize,
    },
}

/// Whether a parsed range can be classified before any input is seen.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum Plan {
    /// Head-relative or statically decidable: classified once, up front.
    Resolved(SlicePlan),
    /// Tail-relative: resolution needs the input length (per input), or a
    /// streaming buffer when the length is unknowable.
    Deferred(DeferredPlan),
}

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum DeferredPlan {
    /// `start:-m…` — head-relative start, tail-relative end.
    Lag {
        start: usize,
        back: NonZeroUsize,
        step: NonZeroUsize,
    },
}

/// Classify absolute (head-relative) bounds into an execution plan.
#[inline]
fn classify(start: usize, end: Option<usize>, step: Option<NonZeroUsize>) -> SlicePlan {
    // Checked before step: no step can select anything from `start >= end`.
    if end.is_some_and(|end| start >= end) {
        return SlicePlan::Empty;
    }
    match step {
        Some(step) if step.get() > 1 => SlicePlan::Stepped { start, end, step },
        _ if start == 0 && end.is_none() => SlicePlan::Copy,
        _ => SlicePlan::Window { start, end },
    }
}

impl SliceRange {
    #[inline]
    pub(crate) fn plan(&self) -> Plan {
        match self.end {
            None => Plan::Resolved(classify(self.start, None, self.step)),
            Some(SliceIndex::FromStart(end)) => {
                Plan::Resolved(classify(self.start, Some(end), self.step))
            }
            Some(SliceIndex::FromEnd(back)) => Plan::Deferred(DeferredPlan::Lag {
                start: self.start,
                back,
                step: self.step.unwrap_or(NonZeroUsize::MIN),
            }),
        }
    }

    /// Render a human-readable description of what this resolved range selects,
    /// without reading any input. `unit` names the elements (e.g. "line").
    pub(crate) fn explain(&self, unit: &str) -> String {
        let step = self.step.map_or(1, NonZeroUsize::get);
        match self.end {
            None => explain_resolved(self.start, None, step, unit),
            Some(SliceIndex::FromStart(end)) => explain_resolved(self.start, Some(end), step, unit),
            Some(SliceIndex::FromEnd(back)) => explain_lag(self.start, back, step, unit),
        }
    }
}

fn explain_resolved(start: usize, end: Option<usize>, step: usize, unit: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("start: {start}\n"));
    match end {
        None => out.push_str("end:   end of input\n"),
        Some(end) => out.push_str(&format!("end:   {end} (exclusive)\n")),
    }
    out.push_str(&format!("step:  {step}\n"));

    match end {
        None => out.push_str(&format!(
            "0-based: {unit}s at indices [{start}, end of input)"
        )),
        Some(end) => out.push_str(&format!("0-based: {unit}s at indices [{start}, {end})")),
    }
    if step != 1 {
        out.push_str(&format!(", every {step} starting at {start}"));
    }
    out.push('\n');

    // 1-based human positions ("Nth line").
    let first_pos = start.saturating_add(1);
    match end {
        None => {
            if step == 1 {
                out.push_str(&format!(
                    "1-based: from the {} {unit} to the last {unit}\n",
                    ordinal(first_pos)
                ));
            } else {
                out.push_str(&format!(
                    "1-based: every {step}{} {unit} from the {} {unit} to the last {unit}\n",
                    ordinal_suffix(step),
                    ordinal(first_pos)
                ));
            }
            out.push_str(&format!("count: until end of input (step {step})"));
        }
        // end is exclusive 0-based, so the last selected 1-based position is `end`.
        Some(end) => {
            if start >= end {
                out.push_str(&format!(
                    "1-based: empty (start {first_pos} is at or past end {end})\n"
                ));
                out.push_str("count: 0");
            } else {
                if step == 1 {
                    out.push_str(&format!(
                        "1-based: from the {} {unit} to the {} {unit}\n",
                        ordinal(first_pos),
                        ordinal(end)
                    ));
                } else {
                    out.push_str(&format!(
                        "1-based: every {step}{} {unit} from the {} {unit} up to the {} {unit}\n",
                        ordinal_suffix(step),
                        ordinal(first_pos),
                        ordinal(end)
                    ));
                }
                let span = end - start;
                let count = span.div_ceil(step);
                out.push_str(&format!("count: {count}"));
            }
        }
    }
    out.push('\n');
    out
}

/// Tail-relative end: the resolved bound is `length - back`, unknowable
/// without reading the input, so positions are described symbolically.
fn explain_lag(start: usize, back: NonZeroUsize, step: usize, unit: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("start: {start}\n"));
    out.push_str(&format!("end:   {back} from the end (exclusive)\n"));
    out.push_str(&format!("step:  {step}\n"));

    out.push_str(&format!(
        "0-based: {unit}s at indices [{start}, length-{back})"
    ));
    if step != 1 {
        out.push_str(&format!(", every {step} starting at {start}"));
    }
    out.push_str(", clamped to the input length\n");

    let first_pos = start.saturating_add(1);
    // The last index the bound admits is length-back-1, i.e. the (back+1)-th
    // position counting back from the end.
    let last_from_end = back.get().saturating_add(1);
    if step == 1 {
        out.push_str(&format!(
            "1-based: from the {} {unit} to the {} {unit} from the end\n",
            ordinal(first_pos),
            ordinal(last_from_end)
        ));
    } else {
        out.push_str(&format!(
            "1-based: every {step}{} {unit} from the {} {unit} up to the {} {unit} from the end\n",
            ordinal_suffix(step),
            ordinal(first_pos),
            ordinal(last_from_end)
        ));
    }
    out.push_str("count: depends on the input length\n");
    out
}

/// "1st", "2nd", "3rd", "4th" ...
fn ordinal(n: usize) -> String {
    format!("{n}{}", ordinal_suffix(n))
}

fn ordinal_suffix(n: usize) -> &'static str {
    match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    }
}

/// Parse one bound of the range. A leading `-` followed by a bare digit string
/// is tail-relative; anything else (`-`, `--1`, `-+1`) keeps the plain-integer
/// parse error so rejection messages stay unchanged.
fn parse_index(s: &str, field: RangeField) -> Result<Option<SliceIndex>, ParseSliceRangeError> {
    match s.parse::<usize>() {
        Ok(v) => Ok(Some(SliceIndex::FromStart(v))),
        Err(err) if *err.kind() == IntErrorKind::Empty => Ok(None),
        Err(source) => {
            if let Some(magnitude) = s.strip_prefix('-') {
                if magnitude.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                    if let Ok(v) = magnitude.parse::<usize>() {
                        return Ok(Some(match NonZeroUsize::new(v) {
                            Some(back) => SliceIndex::FromEnd(back),
                            // Python has no -0: it means the head, not the end.
                            None => SliceIndex::FromStart(0),
                        }));
                    }
                }
            }
            Err(ParseSliceRangeError::InvalidField {
                field,
                value: s.to_owned(),
                source,
            })
        }
    }
}

impl FromStr for SliceRange {
    type Err = ParseSliceRangeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn parse<T: FromStr<Err = ParseIntError>>(
            s: &str,
            field: RangeField,
        ) -> Result<Option<T>, ParseSliceRangeError> {
            match s.parse::<T>() {
                Ok(v) => Ok(Some(v)),
                Err(err) if *err.kind() == IntErrorKind::Empty => Ok(None),
                Err(source) => Err(ParseSliceRangeError::InvalidField {
                    field,
                    value: s.to_owned(),
                    source,
                }),
            }
        }
        let relative_amount = |amount: &str| -> Result<usize, ParseSliceRangeError> {
            parse(amount, RangeField::End)?.ok_or(ParseSliceRangeError::MissingRelativeAmount)
        };

        let mut ptn = s.split(':');
        let start = parse(ptn.next().unwrap_or(""), RangeField::Start)?.unwrap_or(0usize);
        let maybe_end = ptn.next().ok_or(ParseSliceRangeError::MissingColon)?;
        let (start, end) = if let Some(amount) = maybe_end.strip_prefix("+-") {
            let lines = relative_amount(amount)?;
            (
                start.saturating_sub(lines),
                Some(SliceIndex::FromStart(start.saturating_add(lines))),
            )
        } else if let Some(amount) = maybe_end.strip_prefix('+') {
            let lines = relative_amount(amount)?;
            (
                start,
                Some(SliceIndex::FromStart(start.saturating_add(lines))),
            )
        } else {
            (start, parse_index(maybe_end, RangeField::End)?)
        };
        let step = match ptn.next() {
            Some(step) => Some(parse(step, RangeField::Step)?.unwrap_or(NonZeroUsize::MIN)),
            None => None,
        };
        if ptn.next().is_some() {
            return Err(ParseSliceRangeError::TooManyParts);
        }
        Ok(Self { start, end, step })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let slice = SliceRange::from_str("0:1:1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: Some(SliceIndex::FromStart(1)),
                step: NonZeroUsize::new(1),
            }
        );
    }

    #[test]
    fn without_step() {
        let slice = SliceRange::from_str("0:1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: Some(SliceIndex::FromStart(1)),
                step: None,
            }
        );
        let slice = SliceRange::from_str("0:1:").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: Some(SliceIndex::FromStart(1)),
                step: NonZeroUsize::new(1),
            }
        );
    }

    #[test]
    fn without_start() {
        let slice = SliceRange::from_str(":1:1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: Some(SliceIndex::FromStart(1)),
                step: NonZeroUsize::new(1),
            }
        );
    }

    #[test]
    fn without_end() {
        let slice = SliceRange::from_str("0::1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: None,
                step: NonZeroUsize::new(1),
            }
        );
    }

    #[test]
    fn without_start_and_end() {
        let slice = SliceRange::from_str("::1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: None,
                step: NonZeroUsize::new(1),
            }
        );
    }

    #[test]
    fn without_all() {
        let slice = SliceRange::from_str(":").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: None,
                step: None,
            }
        );
        let slice = SliceRange::from_str("::").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: None,
                step: NonZeroUsize::new(1),
            }
        );
    }

    #[test]
    fn default_step_is_one() {
        let slice = SliceRange::from_str("0:1:").expect("parse failed.");
        assert_eq!(slice.step, Some(NonZeroUsize::MIN));
    }

    #[test]
    fn plan_copies_whole_input_ranges() {
        for whole in [":", "::", "0:", "0::", "::1", "0::1"] {
            assert_eq!(
                SliceRange::from_str(whole).unwrap().plan(),
                Plan::Resolved(SlicePlan::Copy),
                "{whole} should select the whole input"
            );
        }
    }

    #[test]
    fn plan_windows_contiguous_subranges() {
        for (range, start, end) in [
            ("1:", 1, None),
            (":1", 0, Some(1)),
            ("5:15", 5, Some(15)),
            ("1::1", 1, None),
            ("1:+3", 1, Some(4)),
            ("5:+-10", 0, Some(15)),
        ] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Window { start, end }),
                "{range} should be a contiguous window"
            );
        }
    }

    #[test]
    fn plan_steps_non_contiguous_selections() {
        for (range, start, end, step) in [("::2", 0, None, 2), ("1:8:2", 1, Some(8), 2)] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Stepped {
                    start,
                    end,
                    step: NonZeroUsize::new(step).unwrap()
                }),
                "{range} must stay on the stepped pipeline"
            );
        }
    }

    #[test]
    fn plan_empties_bounded_start_at_or_past_end() {
        for empty in ["5:3", "5:5", ":0", "5:3:2", "5:+0", ":-0"] {
            assert_eq!(
                SliceRange::from_str(empty).unwrap().plan(),
                Plan::Resolved(SlicePlan::Empty),
                "{empty} selects nothing and must not read input"
            );
        }
    }

    #[test]
    fn plan_keeps_nonempty_bounded_ranges_off_empty() {
        for (range, start, end) in [("0:1", 0, Some(1)), ("4:5", 4, Some(5))] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Window { start, end }),
                "{range} selects something and must stay a window"
            );
        }
    }

    #[test]
    fn plus_sign() {
        let slice = SliceRange::from_str("1:+1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 1,
                end: Some(SliceIndex::FromStart(2)),
                step: None,
            }
        )
    }

    #[test]
    fn plus_minus_sign() {
        let slice = SliceRange::from_str("100:+-10").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 90,
                end: Some(SliceIndex::FromStart(110)),
                step: None,
            }
        )
    }

    #[test]
    fn plus_minus_sign_saturates_start() {
        let slice = SliceRange::from_str("5:+-10").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: Some(SliceIndex::FromStart(15)),
                step: None,
            }
        )
    }

    #[test]
    fn negative_end() {
        assert_eq!(
            SliceRange::from_str(":-2").unwrap(),
            SliceRange {
                start: 0,
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(2).unwrap())),
                step: None,
            }
        );
        assert_eq!(
            SliceRange::from_str("5:-3:2").unwrap(),
            SliceRange {
                start: 5,
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(3).unwrap())),
                step: NonZeroUsize::new(2),
            }
        );
        assert_eq!(
            SliceRange::from_str(":-18446744073709551615").unwrap().end,
            Some(SliceIndex::FromEnd(NonZeroUsize::new(usize::MAX).unwrap()))
        );
    }

    #[test]
    fn minus_zero_normalizes_to_head() {
        assert_eq!(
            SliceRange::from_str(":-0").unwrap().end,
            Some(SliceIndex::FromStart(0))
        );
    }

    #[test]
    fn plan_defers_tail_relative_end() {
        for (range, start, back, step) in [(":-2", 0, 2, 1), ("1:-1", 1, 1, 1), ("5:-3:2", 5, 3, 2)]
        {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Deferred(DeferredPlan::Lag {
                    start,
                    back: NonZeroUsize::new(back).unwrap(),
                    step: NonZeroUsize::new(step).unwrap(),
                }),
                "{range} needs the input length and must defer"
            );
        }
    }

    mod explain {
        use super::*;

        #[test]
        fn bounded_range_shows_count_and_positions() {
            let text = SliceRange::from_str("10:20").unwrap().explain("line");
            assert!(text.contains("start: 10"));
            assert!(text.contains("end:   20 (exclusive)"));
            assert!(text.contains("step:  1"));
            assert!(text.contains("0-based: lines at indices [10, 20)"));
            assert!(text.contains("from the 11th line to the 20th line"));
            assert!(text.contains("count: 10"));
        }

        #[test]
        fn unbounded_end_is_named() {
            let text = SliceRange::from_str(":").unwrap().explain("line");
            assert!(text.contains("end:   end of input"));
            assert!(text.contains("0-based: lines at indices [0, end of input)"));
            assert!(text.contains("to the last line"));
            assert!(text.contains("count: until end of input"));
        }

        #[test]
        fn extended_plus_minus_window_is_resolved() {
            let text = SliceRange::from_str("100:+-10").unwrap().explain("line");
            assert!(text.contains("start: 90"));
            assert!(text.contains("end:   110 (exclusive)"));
            assert!(text.contains("count: 20"));
        }

        #[test]
        fn extended_plus_window_is_resolved() {
            let text = SliceRange::from_str("5:+10").unwrap().explain("line");
            assert!(text.contains("start: 5"));
            assert!(text.contains("end:   15 (exclusive)"));
            assert!(text.contains("count: 10"));
        }

        #[test]
        fn step_count_rounds_up() {
            // indices 10,13,16,19 -> 4 elements
            let text = SliceRange::from_str("10:20:3").unwrap().explain("line");
            assert!(text.contains("step:  3"));
            assert!(text.contains("every 3 starting at 10"));
            assert!(text.contains("count: 4"));
        }

        #[test]
        fn empty_range_reports_zero() {
            let text = SliceRange::from_str("5:5").unwrap().explain("line");
            assert!(text.contains("empty"));
            assert!(text.contains("count: 0"));
        }

        #[test]
        fn max_start_saturates_first_position() {
            let range = SliceRange {
                start: usize::MAX,
                end: None,
                step: None,
            };
            let text = range.explain("line");
            assert!(text.contains("from the 18446744073709551615th line"));
        }

        #[test]
        fn unit_name_is_used() {
            let text = SliceRange::from_str("0:3").unwrap().explain("byte");
            assert!(text.contains("0-based: bytes at indices [0, 3)"));
            assert!(text.contains("from the 1st byte to the 3rd byte"));
        }

        #[test]
        fn negative_end_count_depends_on_length() {
            let text = SliceRange::from_str("2:-2").unwrap().explain("line");
            assert!(text.contains("start: 2"));
            assert!(text.contains("end:   2 from the end (exclusive)"));
            assert!(text
                .contains("0-based: lines at indices [2, length-2), clamped to the input length"));
            assert!(text.contains("1-based: from the 3rd line to the 3rd line from the end"));
            assert!(text.contains("count: depends on the input length"));

            let stepped = SliceRange::from_str("1:-1:2").unwrap().explain("line");
            assert!(stepped.contains("0-based: lines at indices [1, length-1), every 2 starting at 1, clamped to the input length"));
            assert!(stepped.contains(
                "1-based: every 2nd line from the 2nd line up to the 2nd line from the end"
            ));
            assert!(stepped.contains("count: depends on the input length"));
        }
    }

    mod invalid {
        use super::*;

        #[test]
        fn empty() {
            assert!(SliceRange::from_str("").is_err());
        }

        #[test]
        fn non_integer_start() {
            assert!(SliceRange::from_str("a:1").is_err());
            assert!(SliceRange::from_str("a:1:1").is_err());
        }

        #[test]
        fn non_integer_end() {
            assert!(SliceRange::from_str("1:a").is_err());
            assert!(SliceRange::from_str("1:a:1").is_err());
        }

        #[test]
        fn non_integer_step() {
            assert!(SliceRange::from_str("1:1:b").is_err());
        }

        #[test]
        fn missing_colon() {
            let err = SliceRange::from_str("3").expect_err("bare number must be rejected");
            assert_eq!(err, ParseSliceRangeError::MissingColon);
            assert_eq!(
                err.to_string(),
                "range requires a ':' separator (e.g. '3:4', '3:', or ':3')"
            );
        }

        #[test]
        fn too_many_parts() {
            assert!(SliceRange::from_str("1:2:3:4").is_err());
            assert!(SliceRange::from_str("1:2:3:4:5").is_err());
            assert!(SliceRange::from_str(":::").is_err());
        }

        #[test]
        fn invalid_field_names_which_part() {
            assert!(matches!(
                SliceRange::from_str("a:1").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Start,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("1:a").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("1:1:b").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Step,
                    ..
                }
            ));
            assert!(SliceRange::from_str("1:5:x")
                .unwrap_err()
                .to_string()
                .starts_with("invalid step value 'x':"));
        }

        #[test]
        fn relative_end_requires_a_count() {
            assert_eq!(
                SliceRange::from_str("5:+").unwrap_err(),
                ParseSliceRangeError::MissingRelativeAmount
            );
            assert_eq!(
                SliceRange::from_str("5:+-").unwrap_err(),
                ParseSliceRangeError::MissingRelativeAmount
            );
        }

        #[test]
        fn relative_end_rejects_non_numeric_count() {
            assert!(matches!(
                SliceRange::from_str("1:+x").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
        }

        #[test]
        fn bare_minus_rejected() {
            let err = SliceRange::from_str(":-").unwrap_err();
            assert!(matches!(
                err,
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(err.to_string().starts_with("invalid end value '-':"));
        }

        #[test]
        fn double_minus_rejected() {
            assert!(matches!(
                SliceRange::from_str(":--1").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
        }

        #[test]
        fn minus_plus_rejected() {
            assert!(matches!(
                SliceRange::from_str(":-+1").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
        }

        #[test]
        fn negative_end_overflow_rejected() {
            assert!(matches!(
                SliceRange::from_str(":-18446744073709551616").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
        }

        #[test]
        fn negative_step_stays_rejected() {
            for step in ["::-1", "::0"] {
                assert!(
                    matches!(
                        SliceRange::from_str(step).unwrap_err(),
                        ParseSliceRangeError::InvalidField {
                            field: RangeField::Step,
                            ..
                        }
                    ),
                    "{step} must keep rejecting non-positive steps"
                );
            }
        }
    }
}
