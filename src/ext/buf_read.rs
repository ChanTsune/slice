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
        match read_line(&mut self.buf, &mut buf) {
            Ok(0) => None,
            Ok(_n) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }
}

#[inline]
fn read_line<R: BufRead + ?Sized>(r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
    read_until(r, b'\n', buf)
}

#[derive(Clone, Debug)]
pub(crate) struct Delimited<'d, B> {
    buf: B,
    delimiter: &'d [u8],
}

impl<B: BufRead> Iterator for Delimited<'_, B> {
    type Item = io::Result<Vec<u8>>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = Default::default();
        match read_until_delim(&mut self.buf, self.delimiter, &mut buf) {
            Ok(0) => None,
            Ok(_n) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }
}

#[inline]
fn read_until_delim<R: BufRead + ?Sized>(
    r: &mut R,
    delimiter: &[u8],
    buf: &mut Vec<u8>,
) -> io::Result<usize> {
    if let Some(&last) = delimiter.last() {
        loop {
            match read_until(r, last, buf)? {
                0 => return Ok(buf.len()),
                _ if buf.ends_with(delimiter) => return Ok(buf.len()),
                _ => {}
            }
        }
    } else {
        let mut byte = [0; 1];
        match r.read(&mut byte)? {
            0 => Ok(0),
            n => {
                buf.extend_from_slice(&byte[..n]);
                Ok(n)
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
    fn delimit_by(self, delimiter: &[u8]) -> Delimited<'_, Self>
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

fn read_until<R: BufRead + ?Sized>(r: &mut R, delim: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
    let mut read = 0;
    loop {
        let (done, used) = {
            let available = match r.fill_buf() {
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            match memchr::memchr(delim, available) {
                Some(i) => {
                    buf.extend_from_slice(&available[..=i]);
                    (true, i + 1)
                }
                None => {
                    buf.extend_from_slice(available);
                    (false, available.len())
                }
            }
        };
        r.consume(used);
        read += used;
        if done || used == 0 {
            return Ok(read);
        }
    }
}

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

    #[test]
    fn delimit_by_nul() {
        let mut delimited = BufReader::new(&b"a\0b\0"[..]).delimit_by(&[0]);
        assert_eq!(b"a\0", delimited.next().unwrap().unwrap().as_slice());
        assert_eq!(b"b\0", delimited.next().unwrap().unwrap().as_slice());
    }
}
