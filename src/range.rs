use std::{
    num::{IntErrorKind, NonZeroUsize, ParseIntError},
    str::FromStr,
};

/// A resolved `start:end:step` selection over zero-indexed records.
///
/// `end == usize::MAX` marks an omitted end ("to end of input"); direct
/// construction must uphold that convention. `step == None` behaves as a
/// step of 1.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct SliceRange {
    pub start: usize,
    pub end: usize,
    pub step: Option<NonZeroUsize>,
}

impl SliceRange {
    /// Render a human-readable description of what this resolved range selects,
    /// without reading any input. `unit` names the elements (e.g. "line").
    pub fn explain(&self, unit: &str) -> String {
        let step = self.step.map_or(1, NonZeroUsize::get);
        let unbounded = self.end == usize::MAX;

        let mut out = String::new();
        out.push_str(&format!("start: {}\n", self.start));
        if unbounded {
            out.push_str("end:   end of input\n");
        } else {
            out.push_str(&format!("end:   {} (exclusive)\n", self.end));
        }
        out.push_str(&format!("step:  {step}\n"));

        // 0-based selection, end exclusive.
        if unbounded {
            out.push_str(&format!(
                "0-based: {unit}s at indices [{}, end of input)",
                self.start
            ));
        } else {
            out.push_str(&format!(
                "0-based: {unit}s at indices [{}, {})",
                self.start, self.end
            ));
        }
        if step != 1 {
            out.push_str(&format!(", every {step} starting at {}", self.start));
        }
        out.push('\n');

        // 1-based human positions ("Nth line"). `start` can be `usize::MAX`
        // (e.g. "18446744073709551615:"), so saturate instead of overflowing.
        let first_pos = self.start.saturating_add(1);
        if unbounded {
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
        } else {
            let last_pos = self.end; // end is exclusive 0-based, so the last selected 1-based position is `end`
            if self.start >= self.end {
                out.push_str(&format!(
                    "1-based: empty (start {} is at or past end {})\n",
                    first_pos, self.end
                ));
                out.push_str("count: 0");
            } else {
                if step == 1 {
                    out.push_str(&format!(
                        "1-based: from the {} {unit} to the {} {unit}\n",
                        ordinal(first_pos),
                        ordinal(last_pos)
                    ));
                } else {
                    out.push_str(&format!(
                        "1-based: every {step}{} {unit} from the {} {unit} up to the {} {unit}\n",
                        ordinal_suffix(step),
                        ordinal(first_pos),
                        ordinal(last_pos)
                    ));
                }
                let span = self.end - self.start;
                let count = span.div_ceil(step);
                out.push_str(&format!("count: {count}"));
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
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn parse_or<T: FromStr<Err = ParseIntError>>(s: &str, empty: T) -> Result<T, String> {
            let result: Result<T, ParseIntError> = s.parse();
            match result {
                Ok(v) => Ok(v),
                Err(err) if *err.kind() == IntErrorKind::Empty => Ok(empty),
                Err(err) => Err(err),
            }
            .map_err(|e| e.to_string())
        }
        let mut ptn = s.split(':');
        let maybe_start = ptn
            .next()
            .ok_or_else(|| "range start must be needed".to_owned())?;
        let start: usize = parse_or(maybe_start, 0)?;
        let maybe_end = ptn.next().ok_or_else(|| {
            "range requires a ':' separator (e.g. '3:4', '3:', or ':3')".to_owned()
        })?;
        let (start, end) = if let Some(maybe_lines) = maybe_end.strip_prefix("+-") {
            let lines = parse_or(maybe_lines, usize::MAX)?;
            (start.saturating_sub(lines), start.saturating_add(lines))
        } else if let Some(maybe_lines) = maybe_end.strip_prefix('+') {
            let lines = parse_or(maybe_lines, usize::MAX)?;
            (start, start.saturating_add(lines))
        } else {
            (start, parse_or(maybe_end, usize::MAX)?)
        };
        let step = match ptn.next() {
            Some(step) => Some(parse_or(step, NonZeroUsize::MIN)?),
            None => None,
        };
        if ptn.next().is_some() {
            return Err(
                "too many ':' separators in range (expected at most start:end:step)".to_owned(),
            );
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
                end: 1,
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
                end: 1,
                step: None,
            }
        );
        let slice = SliceRange::from_str("0:1:").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: 1,
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
                end: 1,
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
                end: usize::MAX,
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
                end: usize::MAX,
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
                end: usize::MAX,
                step: None,
            }
        );
        let slice = SliceRange::from_str("::").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: usize::MAX,
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
    fn plus_sign() {
        let slice = SliceRange::from_str("1:+1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 1,
                end: 2,
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
                end: 110,
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
                end: 15,
                step: None,
            }
        )
    }

    #[test]
    fn plus_sign_saturates_end() {
        let slice = SliceRange::from_str("5:+").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 5,
                end: usize::MAX,
                step: None,
            }
        )
    }

    #[test]
    fn plus_minus_sign_saturates_both_ends() {
        let slice = SliceRange::from_str("5:+-").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: 0,
                end: usize::MAX,
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
        fn start_at_usize_max_does_not_overflow() {
            let range = SliceRange {
                start: usize::MAX,
                end: usize::MAX,
                step: None,
            };
            let text = range.explain("line");
            assert!(text.contains(&usize::MAX.to_string()));
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
            assert_eq!(
                err,
                "range requires a ':' separator (e.g. '3:4', '3:', or ':3')"
            );
        }

        #[test]
        fn too_many_parts() {
            assert!(SliceRange::from_str("1:2:3:4").is_err());
            assert!(SliceRange::from_str("1:2:3:4:5").is_err());
            assert!(SliceRange::from_str(":::").is_err());
        }
    }
}
