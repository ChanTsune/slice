use std::{
    num::{IntErrorKind, NonZeroUsize, ParseIntError},
    str::FromStr,
};

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SliceRange {
    pub(crate) start: usize,
    /// `None` means unbounded (run to the end of input).
    pub(crate) end: Option<usize>,
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

impl SliceRange {
    /// True when `step` is `None` or `1`: every element survives in order, so a
    /// contiguous copy reproduces the stepped iterator. `is_identity` is the
    /// `start == 0 && end == None` special case of this.
    #[inline]
    pub(crate) fn is_unit_step(&self) -> bool {
        self.step.is_none_or(|step| step.get() == 1)
    }

    /// True when the range selects the whole input unchanged (`:`, `::`, `0::1`):
    /// the output then equals the input byte-for-byte in every mode, so the
    /// splitting pipeline can be skipped in favor of a verbatim copy.
    #[inline]
    pub(crate) fn is_identity(&self) -> bool {
        self.start == 0 && self.end.is_none() && self.is_unit_step()
    }

    /// Render a human-readable description of what this resolved range selects,
    /// without reading any input. `unit` names the elements (e.g. "line").
    pub(crate) fn explain(&self, unit: &str) -> String {
        let step = self.step.map_or(1, NonZeroUsize::get);

        let mut out = String::new();
        out.push_str(&format!("start: {}\n", self.start));
        match self.end {
            None => out.push_str("end:   end of input\n"),
            Some(end) => out.push_str(&format!("end:   {end} (exclusive)\n")),
        }
        out.push_str(&format!("step:  {step}\n"));

        // 0-based selection, end exclusive.
        match self.end {
            None => out.push_str(&format!(
                "0-based: {unit}s at indices [{}, end of input)",
                self.start
            )),
            Some(end) => out.push_str(&format!(
                "0-based: {unit}s at indices [{}, {end})",
                self.start
            )),
        }
        if step != 1 {
            out.push_str(&format!(", every {step} starting at {}", self.start));
        }
        out.push('\n');

        // 1-based human positions ("Nth line").
        let first_pos = self.start.saturating_add(1);
        match self.end {
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
                if self.start >= end {
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
                    let span = end - self.start;
                    let count = span.div_ceil(step);
                    out.push_str(&format!("count: {count}"));
                }
            }
        }
        out.push('\n');
        out
    }
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
                Some(start.saturating_add(lines)),
            )
        } else if let Some(amount) = maybe_end.strip_prefix('+') {
            let lines = relative_amount(amount)?;
            (start, Some(start.saturating_add(lines)))
        } else {
            (start, parse(maybe_end, RangeField::End)?)
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
                end: Some(1),
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
                end: Some(1),
                step: None,
            }
        );
        let slice = SliceRange::from_str("0:1:").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: Some(1),
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
                end: Some(1),
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
    fn is_identity_recognizes_whole_input_ranges() {
        for whole in [":", "::", "0:", "0::", "::1", "0::1"] {
            assert!(
                SliceRange::from_str(whole).unwrap().is_identity(),
                "{whole} should select the whole input"
            );
        }
        for sliced in ["1:", ":1", "::2", "0:5", "1::1", "5:+-10"] {
            assert!(
                !SliceRange::from_str(sliced).unwrap().is_identity(),
                "{sliced} must not be treated as identity"
            );
        }
    }

    #[test]
    fn is_unit_step_recognizes_none_or_one_step() {
        for unit in [":", "::", "0::1", "10:", "5:15", ":15", "1:+3"] {
            assert!(
                SliceRange::from_str(unit).unwrap().is_unit_step(),
                "{unit} should be unit step"
            );
        }
        for stepped in ["::2", "::6", "1:8:2"] {
            assert!(
                !SliceRange::from_str(stepped).unwrap().is_unit_step(),
                "{stepped} must not be treated as unit step"
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
                end: Some(2),
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
                end: Some(110),
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
                end: Some(15),
                step: None,
            }
        )
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
    }
}
