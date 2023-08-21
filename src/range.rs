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
        let ptn = s.split(':').collect::<Vec<_>>();
        Ok(Self {
            start: parse_or(
                ptn.first()
                    .ok_or_else(|| String::from("range start must be needed"))?,
                0,
            )?,
            end: parse_or(
                ptn.get(1)
                    .ok_or_else(|| String::from("range end must be needed"))?,
                usize::MAX,
            )?,
            step: match ptn.get(2) {
                Some(step) => Some(parse_or(step, unsafe { NonZeroUsize::new_unchecked(1) })?),
                None => None,
            },
        })
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
    }
}
