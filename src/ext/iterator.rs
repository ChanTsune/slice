use std::{
    iter::{Skip, StepBy, Take},
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

pub(crate) trait IteratorExt {
    #[inline]
    fn slice(self, start: usize, stop: usize, skip: Option<NonZeroUsize>) -> Slice<Self>
    where
        Self: Sized + Iterator,
    {
        Slice::new(self, start, stop, skip)
    }
}

impl<I: Iterator> IteratorExt for I {}
