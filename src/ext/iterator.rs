use std::{
    iter::{Enumerate, Skip, StepBy, Take},
    num::NonZeroUsize,
};

#[derive(Debug)]
pub(crate) struct Slice<R>(StepBy<Skip<Take<R>>>);

impl<R: Iterator> Slice<R> {
    #[inline]
    fn new(r: R, start: usize, end: usize, step: Option<NonZeroUsize>) -> Self {
        Self(
            r.take(end)
                .skip(start)
                .step_by(step.map(|step| step.get()).unwrap_or(1)),
        )
    }
}

impl<I: Iterator> Iterator for Slice<I> {
    type Item = <I as Iterator>::Item;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

#[derive(Debug)]
pub(crate) struct ExcludeSlice<R> {
    iter: Enumerate<R>,
    start: usize,
    end: usize,
    step: usize,
}

impl<R: Iterator> ExcludeSlice<R> {
    #[inline]
    fn new(r: R, start: usize, end: usize, step: Option<NonZeroUsize>) -> Self {
        Self {
            iter: r.enumerate(),
            start,
            end,
            step: step.map(|step| step.get()).unwrap_or(1),
        }
    }

    #[inline]
    fn excludes(&self, index: usize) -> bool {
        self.start <= index && index < self.end && (index - self.start) % self.step == 0
    }
}

impl<I: Iterator> Iterator for ExcludeSlice<I> {
    type Item = <I as Iterator>::Item;
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (index, item) = self.iter.next()?;
            if !self.excludes(index) {
                return Some(item);
            }
        }
    }
}

pub(crate) trait IteratorExt {
    #[inline]
    fn slice(self, start: usize, stop: usize, skip: Option<NonZeroUsize>) -> Slice<Self>
    where
        Self: Sized + Iterator,
    {
        Slice::new(self, start, stop, skip)
    }

    #[inline]
    fn exclude_slice(
        self,
        start: usize,
        stop: usize,
        skip: Option<NonZeroUsize>,
    ) -> ExcludeSlice<Self>
    where
        Self: Sized + Iterator,
    {
        ExcludeSlice::new(self, start, stop, skip)
    }
}

impl<I: Iterator> IteratorExt for I {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_middle_range() {
        let items: Vec<_> = (0..6).exclude_slice(2, 5, None).collect();

        assert_eq!(items, vec![0, 1, 5]);
    }

    #[test]
    fn exclude_respects_step() {
        let items: Vec<_> = (0..8).exclude_slice(1, 7, NonZeroUsize::new(2)).collect();

        assert_eq!(items, vec![0, 2, 4, 6, 7]);
    }

    #[test]
    fn exclude_with_start_after_end_removes_nothing() {
        let items: Vec<_> = (0..4).exclude_slice(3, 1, None).collect();

        assert_eq!(items, vec![0, 1, 2, 3]);
    }
}
