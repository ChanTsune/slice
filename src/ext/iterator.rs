use crate::range::SliceRanges;

#[derive(Debug)]
pub(crate) struct Slice<'r, R> {
    iter: R,
    ranges: &'r SliceRanges,
    range_index: usize,
    index: usize,
}

impl<'r, R: Iterator> Slice<'r, R> {
    #[inline]
    fn new(iter: R, ranges: &'r SliceRanges) -> Self {
        Self {
            iter,
            ranges,
            range_index: 0,
            index: 0,
        }
    }
}

impl<I: Iterator> Iterator for Slice<'_, I> {
    type Item = <I as Iterator>::Item;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let range = self.ranges.as_slice().get(self.range_index)?;
            if self.index >= range.end {
                self.range_index += 1;
                continue;
            }

            let item = self.iter.next()?;
            let index = self.index;
            self.index = self.index.saturating_add(1);

            let step = range.step.map(|step| step.get()).unwrap_or(1);
            if index >= range.start && (index - range.start) % step == 0 {
                return Some(item);
            }
        }
    }
}

pub(crate) trait IteratorExt {
    #[inline]
    fn slice(self, ranges: &SliceRanges) -> Slice<'_, Self>
    where
        Self: Sized + Iterator,
    {
        Slice::new(self, ranges)
    }
}

impl<I: Iterator> IteratorExt for I {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn slice() {
        let ranges = SliceRanges::from_str("0:3,6:9").unwrap();
        let actual = (0..10).slice(&ranges).collect::<Vec<_>>();
        assert_eq!(actual, vec![0, 1, 2, 6, 7, 8]);
    }

    #[test]
    fn slice_with_step() {
        let ranges = SliceRanges::from_str("0:6:2,6:9").unwrap();
        let actual = (0..10).slice(&ranges).collect::<Vec<_>>();
        assert_eq!(actual, vec![0, 2, 4, 6, 7, 8]);
    }
}
