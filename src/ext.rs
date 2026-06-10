mod buf_read;
mod iterator;

pub(crate) use buf_read::{slice_stepped, slice_window, Byte, Bytes, PerByte};
pub(crate) use iterator::IteratorExt;
