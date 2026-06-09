use std::io::{self, BufRead, Write};

/// Scan to the first `delim` byte. `sink` receives each consumed slice; it is the
/// only thing that differs between reading (copy into a buffer) and skipping
/// (discard). Returns the total bytes consumed: `Ok(0)` means the stream was
/// already at EOF (no chunk), `Ok(n > 0)` means a chunk was consumed — including
/// a final chunk that lacks a trailing `delim`.
#[inline]
fn scan_until<R: BufRead + ?Sized>(
    r: &mut R,
    delim: u8,
    mut sink: impl FnMut(&[u8]),
) -> io::Result<usize> {
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
                    sink(&available[..=i]);
                    (true, i + 1)
                }
                None => {
                    sink(available);
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

#[inline]
fn read_until<R: BufRead + ?Sized>(r: &mut R, delim: u8, buf: &mut Vec<u8>) -> io::Result<usize> {
    scan_until(r, delim, |chunk| buf.extend_from_slice(chunk))
}

#[inline]
fn skip_until<R: BufRead + ?Sized>(r: &mut R, delim: u8) -> io::Result<usize> {
    scan_until(r, delim, |_| {})
}

/// A strategy for cutting a stream into chunks. Each chunk keeps its trailing
/// delimiter; the last chunk may lack one. `read` and `skip` are symmetric: both
/// advance the reader by exactly one chunk and return its byte length (`Ok(0)`
/// only at end of stream); `read` additionally appends the chunk to `buf`. Kinds
/// are separate types so the single-byte path never pays for the multi-byte
/// straddle machinery.
pub(crate) trait Split {
    fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize>;
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize>;
}

/// Single-byte delimiter. Lines are `Byte(b'\n')`, `-z` is `Byte(0)`.
pub(crate) struct Byte(pub u8);

/// Multi-byte delimiter (`self.0.len() >= 2`; constructed only by the
/// delimiter-shape dispatch).
pub(crate) struct Bytes<'d>(pub &'d [u8]);

/// Empty delimiter: one byte per chunk.
pub(crate) struct PerByte;

impl Split for Byte {
    #[inline]
    fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
        read_until(r, self.0, buf)
    }
    #[inline]
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
        skip_until(r, self.0)
    }
}

impl Split for PerByte {
    #[inline]
    fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
        let mut byte = [0; 1];
        match r.read(&mut byte)? {
            0 => Ok(0),
            n => {
                buf.extend_from_slice(&byte[..n]);
                Ok(n)
            }
        }
    }
    #[inline]
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
        loop {
            let available = match r.fill_buf() {
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if available.is_empty() {
                return Ok(0);
            }
            r.consume(1);
            return Ok(1);
        }
    }
}

impl Split for Bytes<'_> {
    #[inline]
    fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
        let last = *self.0.last().expect("Bytes delimiter is non-empty");
        loop {
            match read_until(r, last, buf)? {
                0 => return Ok(buf.len()),
                _ if buf.ends_with(self.0) => return Ok(buf.len()),
                _ => {}
            }
        }
    }

    #[inline]
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
        skip_until_delim(r, self.0)
    }
}

/// Skip one multi-byte-delimited chunk without retaining the chunk.
///
/// A delimiter can straddle a `fill_buf` boundary, so we carry a rolling `tail`
/// of the bytes seen *within the current chunk* and reproduce `read`'s
/// `buf.ends_with(delimiter)` check against `tail`. Two invariants make this
/// match `Bytes::read` exactly:
///   * the tail only ever holds bytes belonging to the current chunk, so a match
///     can never span a previous chunk boundary, and
///   * on a confirmed match the chunk ends and the tail is CLEARED — the matched
///     delimiter's bytes belong to the just-ended chunk and must not seed the
///     next chunk's match window (e.g. `aaaaaa` with `aaa` -> `aaa`, `aaa`).
///
/// To bound memory the tail is trimmed to the last `delimiter.len() - 1` bytes
/// after each non-matching scan (a longer prefix can never complete a match).
fn skip_until_delim<R: BufRead + ?Sized>(r: &mut R, delimiter: &[u8]) -> io::Result<usize> {
    let last = *delimiter.last().expect("multi-byte delimiter is non-empty");
    let keep = delimiter.len() - 1;
    let mut tail: Vec<u8> = Vec::with_capacity(keep);
    let mut read = 0usize;
    loop {
        let (done, used) = {
            let available = match r.fill_buf() {
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            match memchr::memchr(last, available) {
                Some(i) => {
                    if ends_with_across(&tail, &available[..=i], delimiter) {
                        (true, i + 1)
                    } else {
                        extend_tail(&mut tail, &available[..=i], keep);
                        (false, i + 1)
                    }
                }
                None => {
                    extend_tail(&mut tail, available, keep);
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

/// True iff `(tail ++ recent)` ends with `delimiter`. `recent` is the slice just
/// scanned (ending at the candidate `last` byte); `tail` holds earlier bytes of
/// the same chunk. Compares only the trailing `delimiter.len()` bytes.
#[inline]
fn ends_with_across(tail: &[u8], recent: &[u8], delimiter: &[u8]) -> bool {
    let need = delimiter.len();
    if tail.len() + recent.len() < need {
        return false;
    }
    let from_recent = recent.len().min(need);
    let from_tail = need - from_recent;
    if recent[recent.len() - from_recent..] != delimiter[need - from_recent..] {
        return false;
    }
    from_tail == 0 || tail[tail.len() - from_tail..] == delimiter[..from_tail]
}

/// Append `chunk` to `tail`, keeping only the trailing `keep` bytes (the most a
/// future delimiter match could need from before the current scan position).
#[inline]
fn extend_tail(tail: &mut Vec<u8>, chunk: &[u8], keep: usize) {
    if keep == 0 {
        return;
    }
    if chunk.len() >= keep {
        tail.clear();
        tail.extend_from_slice(&chunk[chunk.len() - keep..]);
    } else {
        let overflow = (tail.len() + chunk.len()).saturating_sub(keep);
        tail.drain(..overflow);
        tail.extend_from_slice(chunk);
    }
}

#[derive(Debug)]
pub(crate) struct Chunks<S, B> {
    split: S,
    buf: B,
}

impl<S: Split, B: BufRead> Iterator for Chunks<S, B> {
    type Item = io::Result<Vec<u8>>;

    #[inline]
    fn next(&mut self) -> Option<io::Result<Vec<u8>>> {
        let mut buf = Vec::new();
        match self.split.read(&mut self.buf, &mut buf) {
            Ok(0) => None,
            Ok(_n) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }
}

pub(crate) trait BufReadExt: BufRead + Sized {
    #[inline]
    fn lines_with_eol(self) -> Chunks<Byte, Self> {
        Chunks {
            split: Byte(b'\n'),
            buf: self,
        }
    }

    #[inline]
    fn split_chunks<S: Split>(self, split: S) -> Chunks<S, Self> {
        Chunks { split, buf: self }
    }
}

impl<B: BufRead> BufReadExt for B {}

/// Unit-step line/delimiter fast path. A unit-step range selects contiguous
/// chunks, i.e. one contiguous byte span: skip `start` chunks, then emit the
/// window. Unbounded `start:` copies the tail verbatim; bounded `start:end`
/// emits `end - start` more chunks reusing one buffer. `end` is an absolute
/// chunk index from the stream start, matching IteratorExt::slice's
/// take(end).skip(start) ordering, so start >= end yields an empty window.
pub(crate) fn slice_window<S: Split, R: BufRead, W: Write>(
    split: S,
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
) -> io::Result<()> {
    for _ in 0..start {
        if split.skip(&mut input)? == 0 {
            break;
        }
    }
    match end {
        None => {
            io::copy(&mut input, &mut output)?;
        }
        Some(end) => {
            let count = end.saturating_sub(start);
            let mut buf = Vec::new();
            for _ in 0..count {
                buf.clear();
                if split.read(&mut input, &mut buf)? == 0 {
                    break;
                }
                output.write_all(&buf)?;
            }
        }
    }
    output.flush()
}

#[cfg(test)]
trait DelimitBy: BufRead + Sized + 'static {
    /// Test-only convenience that routes a raw delimiter through the
    /// `[] / &[b] / multi` shape match into `split_chunks`, so the existing
    /// iterator round-trip tests keep pinning chunk boundaries.
    fn delimit_by(self, delimiter: &[u8]) -> Box<dyn Iterator<Item = io::Result<Vec<u8>>>> {
        match delimiter {
            [] => Box::new(self.split_chunks(PerByte)),
            &[b] => Box::new(self.split_chunks(Byte(b))),
            multi => Box::new(self.split_chunks(Bytes(multi.to_vec().leak()))),
        }
    }
}

#[cfg(test)]
impl<B: BufRead + 'static> DelimitBy for B {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::IteratorExt;
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

    // Reference: the chunks `split_chunks(split).slice(start, end, None)` would
    // emit, concatenated. `slice_window` must reproduce this byte-for-byte.
    fn reference<S: Split>(split: S, input: &[u8], start: usize, end: Option<usize>) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in BufReader::new(input)
            .split_chunks(split)
            .slice(start, end, None)
        {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    fn windowed<S: Split>(
        split: S,
        input: &[u8],
        start: usize,
        end: Option<usize>,
        capacity: usize,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        slice_window(
            split,
            BufReader::with_capacity(capacity, input),
            &mut out,
            start,
            end,
        )
        .unwrap();
        out
    }

    const RANGES: &[(usize, Option<usize>)] = &[
        (1, None),
        (0, Some(1)),
        (1, Some(3)),
        (0, Some(9)),
        (2, None),
        (200, None),
        (0, Some(0)),
        (3, Some(1)),
    ];

    fn assert_skip_parity<S: Split + Copy>(split: S, input: &[u8]) {
        for &(start, end) in RANGES {
            assert_eq!(
                windowed(split, input, start, end, 8 * 1024),
                reference(split, input, start, end),
                "slice_window diverged from the iterator for {start}:{end:?} on {input:?}"
            );
        }
    }

    #[derive(Clone, Copy)]
    struct ByteRef(u8);
    impl Split for ByteRef {
        fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
            Byte(self.0).read(r, buf)
        }
        fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
            Byte(self.0).skip(r)
        }
    }

    #[derive(Clone, Copy)]
    struct BytesRef<'d>(&'d [u8]);
    impl Split for BytesRef<'_> {
        fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
            Bytes(self.0).read(r, buf)
        }
        fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
            Bytes(self.0).skip(r)
        }
    }

    #[derive(Clone, Copy)]
    struct PerByteRef;
    impl Split for PerByteRef {
        fn read<R: BufRead + ?Sized>(&self, r: &mut R, buf: &mut Vec<u8>) -> io::Result<usize> {
            PerByte.read(r, buf)
        }
        fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
            PerByte.skip(r)
        }
    }

    #[test]
    fn skip_parity_line() {
        assert_skip_parity(ByteRef(b'\n'), b"a\nb\nc\nd\ne\n");
        assert_skip_parity(ByteRef(b'\n'), b"a\nb\nc"); // no trailing newline
    }

    #[test]
    fn skip_parity_comma() {
        assert_skip_parity(ByteRef(b','), b"a,b,c,d,e,");
    }

    #[test]
    fn skip_parity_nul() {
        assert_skip_parity(ByteRef(0), b"a\0b\0c\0d\0");
    }

    #[test]
    fn skip_parity_multibyte() {
        assert_skip_parity(BytesRef(b"||"), b"a||b||c||d||");
        assert_skip_parity(BytesRef(b"||"), b"a||b||c"); // no trailing delimiter
    }

    #[test]
    fn skip_parity_per_byte() {
        assert_skip_parity(PerByteRef, b"abcdef");
    }

    #[test]
    fn skip_until_delim_overlap() {
        // Skipping 1 chunk of `a|||b|` split on `||` yields the rest, `|b|`.
        assert_eq!(
            windowed(BytesRef(b"||"), b"a|||b|", 1, None, 8 * 1024),
            b"|b|"
        );
        // Reading 1 chunk yields the first, `a||`.
        assert_eq!(
            windowed(BytesRef(b"||"), b"a|||b|", 0, Some(1), 8 * 1024),
            b"a||"
        );
    }

    #[test]
    fn skip_until_delim_contiguous_repeat() {
        // The tail must be cleared on a confirmed match: `aaaaaa` splits into
        // `aaa`,`aaa`, so skipping one chunk leaves `aaa` (not `a`).
        assert_eq!(
            windowed(BytesRef(b"aaa"), b"aaaaaa", 1, None, 8 * 1024),
            b"aaa"
        );
        assert_eq!(
            windowed(BytesRef(b"aaa"), b"aaaaaa", 0, Some(1), 8 * 1024),
            b"aaa"
        );
        // `aaaa` splits into `aaa`,`a`.
        assert_eq!(windowed(BytesRef(b"aaa"), b"aaaa", 1, None, 8 * 1024), b"a");
    }

    #[test]
    fn skip_until_delim_straddle() {
        // capacity 1 forces every delimiter to straddle a fill_buf boundary.
        for input in [
            &b"a||b||c||d||"[..],
            &b"a||b||c"[..],
            &b"a|||b|"[..],
            &b"aaaaaa"[..],
            &b"aaaa"[..],
        ] {
            for &(start, end) in RANGES {
                let delim: &[u8] = if input.contains(&b'a') && input.iter().all(|&b| b == b'a') {
                    b"aaa"
                } else {
                    b"||"
                };
                assert_eq!(
                    windowed(BytesRef(delim), input, start, end, 1),
                    reference(BytesRef(delim), input, start, end),
                    "straddle diverged for {start}:{end:?} on {input:?} delim {delim:?}"
                );
            }
        }
    }

    #[test]
    fn slice_window_binary() {
        // Last chunk lacks a trailing newline; the tail must round-trip exact.
        let input = b"slice\xaabin\nslice\xaa";
        assert_eq!(
            windowed(ByteRef(b'\n'), input, 1, None, 8 * 1024),
            b"slice\xaa"
        );
    }

    #[test]
    fn ends_with_across_cases() {
        assert!(!ends_with_across(b"", b"a|", b"||"));
        assert!(ends_with_across(b"|", b"|", b"||"));
        assert!(ends_with_across(b"xy", b"z", b"yz"));
        assert!(!ends_with_across(b"", b"z", b"yz")); // under length
                                                      // recent suffix matches the delimiter tail, but the tail prefix does not.
        assert!(!ends_with_across(b"ay", b"z", b"xyz"));
    }
}
