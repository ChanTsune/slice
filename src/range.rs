use std::{
    num::{IntErrorKind, NonZeroUsize, ParseIntError},
    str::FromStr,
};

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SliceRanges(Vec<SliceRange>);

impl SliceRanges {
    #[inline]
    pub(crate) fn as_slice(&self) -> &[SliceRange] {
        &self.0
    }

    fn validate_stream_order(ranges: &[SliceRange]) -> Result<(), String> {
        let mut consumed = 0;
        for range in ranges {
            if range.start < range.end && range.start < consumed {
                return Err(
                    "range list must be in streaming order; overlapping or backward output ranges require buffering"
                        .to_owned(),
                );
            }
            consumed = consumed.max(range.end);
        }
        Ok(())
    }
}

impl FromStr for SliceRanges {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ranges = s
            .split(',')
            .map(|range| {
                if range.is_empty() {
                    Err("empty range in comma-separated range list".to_owned())
                } else {
                    SliceRange::from_str(range)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::validate_stream_order(&ranges)?;
        Ok(Self(ranges))
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SliceRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) step: Option<NonZeroUsize>,
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

    mod ranges {
        use super::*;

        #[test]
        fn single() {
            let ranges = SliceRanges::from_str("0:1:1").expect("parse failed.");
            assert_eq!(
                ranges.as_slice(),
                &[SliceRange {
                    start: 0,
                    end: 1,
                    step: NonZeroUsize::new(1),
                }]
            );
        }

        #[test]
        fn multiple() {
            let ranges = SliceRanges::from_str("0:5,10:15").expect("parse failed.");
            assert_eq!(
                ranges.as_slice(),
                &[
                    SliceRange {
                        start: 0,
                        end: 5,
                        step: None,
                    },
                    SliceRange {
                        start: 10,
                        end: 15,
                        step: None,
                    }
                ]
            );
        }

        #[test]
        fn rejects_empty_part() {
            assert!(SliceRanges::from_str("0:5,").is_err());
            assert!(SliceRanges::from_str(",0:5").is_err());
        }

        #[test]
        fn rejects_backward_output_range() {
            let err = SliceRanges::from_str("10:15,0:5").expect_err("range list must fail");
            assert!(err.contains("streaming order"));
        }

        #[test]
        fn rejects_overlapping_output_range() {
            let err = SliceRanges::from_str("0:5,3:7").expect_err("range list must fail");
            assert!(err.contains("streaming order"));
        }

        #[test]
        fn allows_step_range_followed_at_end_boundary() {
            let ranges = SliceRanges::from_str("0:6:2,6:7").expect("parse failed.");
            assert_eq!(ranges.as_slice().len(), 2);
        }

        #[test]
        fn rejects_step_range_followed_before_end_boundary() {
            let err = SliceRanges::from_str("0:6:2,5:7").expect_err("range list must fail");
            assert!(err.contains("streaming order"));
        }

        #[test]
        fn allows_empty_range_before_output_range() {
            let ranges = SliceRanges::from_str("0:0,0:2").expect("parse failed.");
            assert_eq!(ranges.as_slice().len(), 2);
        }

        #[test]
        fn allows_empty_range_between_output_ranges() {
            let ranges = SliceRanges::from_str("0:2,1:1,2:3").expect("parse failed.");
            assert_eq!(ranges.as_slice().len(), 3);
        }

        #[test]
        fn allows_empty_backward_range() {
            let ranges = SliceRanges::from_str("10:15,0:0").expect("parse failed.");
            assert_eq!(ranges.as_slice().len(), 2);
        }
    }

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
