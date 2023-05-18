use std::{
    iter::{Skip, StepBy, Take},
    num::NonZeroUsize,
};

#[derive(Debug)]
pub(crate) struct Slice<R> {
    r: StepBy<Skip<Take<R>>>,
}

impl<R: Iterator> Slice<R> {
    fn new(r: R, start: usize, end: usize, step: Option<NonZeroUsize>) -> Self {
        Self {
            r: r.take(end)
                .skip(start)
                .step_by(step.map(|step| step.get()).unwrap_or(1)),
        }
    }
}

impl<I: Iterator> Iterator for Slice<I> {
    type Item = <I as Iterator>::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.r.next()
    }
}

pub(crate) trait IteratorExt {
    fn slice(self, start: usize, stop: usize, skip: Option<NonZeroUsize>) -> Slice<Self>
    where
        Self: Sized + Iterator,
    {
        Slice::new(self, start, stop, skip)
    }
}

impl<I: Iterator> IteratorExt for I {}
