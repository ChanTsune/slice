use std::{
    num::{IntErrorKind, NonZeroUsize, ParseIntError},
    str::FromStr,
};

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
