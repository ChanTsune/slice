use std::io::{self, BufRead};

#[derive(Debug)]
pub(crate) struct LinesWithEol<B> {
    buf: B,
}

impl<B: BufRead> Iterator for LinesWithEol<B> {
    type Item = io::Result<Vec<u8>>;

    #[inline]
    fn next(&mut self) -> Option<io::Result<Vec<u8>>> {
        let mut buf = Default::default();
        match self.buf.read_until(b'\n', &mut buf) {
            Ok(0) => None,
            Ok(_n) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Delimited<'d, B> {
    buf: B,
    delimiter: &'d [u8],
}

impl<'d, B: BufRead> Iterator for Delimited<'d, B> {
    type Item = io::Result<Vec<u8>>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(last) = self.delimiter.last() {
            let mut buf = Default::default();
            loop {
                match self.buf.read_until(*last, &mut buf) {
                    Ok(0) => return if buf.is_empty() { None } else { Some(Ok(buf)) },
                    Ok(_n) => {
                        if buf.ends_with(self.delimiter) {
                            return Some(Ok(buf));
                        }
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
        } else {
            let mut buf = [0; 1];
            match self.buf.read(&mut buf) {
                Ok(0) => None,
                Ok(_) => Some(Ok(Vec::from(buf))),
                Err(e) => Some(Err(e)),
            }
        }
    }
}

pub(crate) trait BufReadExt {
    #[inline]
    fn lines_with_eol(self) -> LinesWithEol<Self>
    where
        Self: Sized,
    {
        LinesWithEol { buf: self }
    }

    #[inline]
    fn delimit_by(self, delimiter: &[u8]) -> Delimited<Self>
    where
        Self: Sized,
    {
        Delimited {
            buf: self,
            delimiter,
        }
    }
}

impl<B: BufRead> BufReadExt for B {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn empty_lines_with_eol() {
        let mut lines = BufReader::new(&b""[..]).lines_with_eol();
        assert!(lines.next().is_none());
    }

    #[test]
    fn lines_with_eol() {
        let mut lines = BufReader::new(&b"1\n2\n"[..]).lines_with_eol();
        assert_eq!(b"1\n", lines.next().unwrap().unwrap().as_slice());
        assert_eq!(b"2\n", lines.next().unwrap().unwrap().as_slice());
    }

    #[test]
    fn lines_without_eol() {
        let mut lines = BufReader::new(&b"1\n2"[..]).lines_with_eol();
        assert_eq!(b"1\n", lines.next().unwrap().unwrap().as_slice());
        assert_eq!(b"2", lines.next().unwrap().unwrap().as_slice());
    }

    #[test]
    fn empty_delimit_by_empty() {
        let mut delimited = BufReader::new(&b""[..]).delimit_by(&b""[..]);
        assert!(delimited.next().is_none());
    }

    #[test]
    fn empty_delimit_by_character() {
        let mut delimited = BufReader::new(&b""[..]).delimit_by(&b"|"[..]);
        assert!(delimited.next().is_none());
    }

    #[test]
    fn empty_delimit_by_string() {
        let mut delimited = BufReader::new(&b""[..]).delimit_by(&b"||"[..]);
        assert!(delimited.next().is_none());
    }

    #[test]
    fn delimit_by_empty() {
        let mut delimited = BufReader::new(&b"a|b|"[..]).delimit_by(&b""[..]);
        assert_eq!(b"a", delimited.next().unwrap().unwrap().as_slice());
        assert_eq!(b"|", delimited.next().unwrap().unwrap().as_slice());
        assert_eq!(b"b", delimited.next().unwrap().unwrap().as_slice());
        assert_eq!(b"|", delimited.next().unwrap().unwrap().as_slice());
    }

    #[test]
    fn delimit_by_character() {
        let mut delimited = BufReader::new(&b"a|b|"[..]).delimit_by(&b"|"[..]);
        assert_eq!(b"a|", delimited.next().unwrap().unwrap().as_slice());
        assert_eq!(b"b|", delimited.next().unwrap().unwrap().as_slice());
    }

    #[test]
    fn delimit_by_string() {
        let mut delimited = BufReader::new(&b"a|||b|"[..]).delimit_by(&b"||"[..]);
        assert_eq!(b"a||", delimited.next().unwrap().unwrap().as_slice());
        assert_eq!(b"|b|", delimited.next().unwrap().unwrap().as_slice());
    }
}
