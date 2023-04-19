use std::io;
use std::num::ParseIntError;
use std::str::FromStr;

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SliceRange {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) step: usize,
}

impl FromStr for SliceRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let ptn = s.split(":").collect::<Vec<_>>();
        Ok(Self {
            start: ptn
                .get(0)
                .unwrap()
                .parse()
                .map_err(|e: ParseIntError| e.to_string())?,
            end: ptn
                .get(1)
                .unwrap()
                .parse()
                .map_err(|e: ParseIntError| e.to_string())?,
            step: ptn
                .get(2)
                .unwrap()
                .parse()
                .map_err(|e: ParseIntError| e.to_string())?,
        })
    }
}
