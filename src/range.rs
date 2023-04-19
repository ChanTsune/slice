use std::{
    num::{NonZeroUsize, ParseIntError},
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
        let ptn = s.split(':').collect::<Vec<_>>();
        Ok(Self {
            start: ptn
                .first()
                .ok_or_else(|| String::from("range start must be needed"))?
                .parse()
                .map_err(|e: ParseIntError| e.to_string())?,
            end: ptn
                .get(1)
                .ok_or_else(|| String::from("range end must be needed"))?
                .parse()
                .map_err(|e: ParseIntError| e.to_string())?,
            step: match ptn.get(2) {
                Some(step) => Some(step.parse().map_err(|e: ParseIntError| e.to_string())?),
                None => None,
            },
        })
    }
}
