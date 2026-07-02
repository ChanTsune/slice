mod buf_read;
mod iterator;
mod utf8;

pub(crate) use buf_read::{
    read_all_with_record_limit, slice_lag, slice_lag_with_record_limit, slice_stepped, slice_tail,
    slice_tail_with_record_limit, slice_window, Byte, Bytes,
};
pub(crate) use iterator::IteratorExt;
pub(crate) use utf8::{char_lag, char_stepped, char_tail, char_window, Utf8Elements};
