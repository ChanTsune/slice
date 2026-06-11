mod buf_read;
mod iterator;

pub(crate) use buf_read::{slice_lag, slice_stepped, slice_tail, slice_window, Byte, Bytes};
pub(crate) use iterator::IteratorExt;
