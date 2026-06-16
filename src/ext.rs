mod buf_read;
mod iterator;

pub(crate) use buf_read::{
    read_all_with_record_limit, slice_lag, slice_lag_with_record_limit, slice_stepped, slice_tail,
    slice_tail_with_record_limit, slice_window, Byte, Bytes,
};
pub(crate) use iterator::IteratorExt;
