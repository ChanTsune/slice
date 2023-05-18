use std::io::{self, BufRead};

#[derive(Debug)]
pub(crate) struct LinesWithEol<B> {
    buf: B,
}

impl<B: BufRead> Iterator for LinesWithEol<B> {
    type Item = io::Result<String>;

    fn next(&mut self) -> Option<io::Result<String>> {
        let mut buf = String::new();
        match self.buf.read_line(&mut buf) {
            Ok(0) => None,
            Ok(_n) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }
}

pub(crate) trait BufReadExt {
    fn lines_with_eol(self) -> LinesWithEol<Self>
    where
        Self: Sized,
    {
        LinesWithEol { buf: self }
    }
}

impl<B: BufRead> BufReadExt for B {}
