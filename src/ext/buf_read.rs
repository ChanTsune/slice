use crate::ext::IteratorExt;
use std::{
    io::{self, BufRead, Write},
    num::NonZeroUsize,
};

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

/// Stepped (step > 1) line/delimiter path. The surviving chunk indices come
/// from running `IteratorExt::slice` over the index stream itself, so the
/// take(end).skip(start).step_by(step) ordering is inherited from the same
/// adapter as every other mode rather than re-derived. Survivors are read into
/// one reused buffer; the gaps between them are skipped without copying.
/// Stops at end of stream or after the last selected index, so a bounded `end`
/// never reads past its final selected chunk.
pub(crate) fn slice_stepped<S: Split, R: BufRead, W: Write>(
    split: S,
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
    step: NonZeroUsize,
) -> io::Result<()> {
    let mut buf = Vec::new();
    let mut index = 0usize;
    'chunks: for target in (0usize..).slice(start, end, Some(step)) {
        while index < target {
            if split.skip(&mut input)? == 0 {
                break 'chunks;
            }
            index += 1;
        }
        buf.clear();
        if split.read(&mut input, &mut buf)? == 0 {
            break;
        }
        output.write_all(&buf)?;
        index += 1;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn empty_lines_with_eol() {
        assert!(chunks(Byte(b'\n'), b"").is_empty());
    }

    #[test]
    fn lines_with_eol() {
        assert_eq!(chunks(Byte(b'\n'), b"1\n2\n"), [b"1\n", b"2\n"]);
    }

    #[test]
    fn lines_without_eol() {
        assert_eq!(chunks(Byte(b'\n'), b"1\n2"), [&b"1\n"[..], b"2"]);
    }

    #[test]
    fn empty_delimit_by_character() {
        assert!(delimited(b"", b"|").is_empty());
    }

    #[test]
    fn empty_delimit_by_string() {
        assert!(delimited(b"", b"||").is_empty());
    }

    #[test]
    fn delimit_by_character() {
        assert_eq!(delimited(b"a|b|", b"|"), [b"a|", b"b|"]);
    }

    #[test]
    fn delimit_by_string() {
        assert_eq!(delimited(b"a|||b|", b"||"), [b"a||", b"|b|"]);
    }

    #[test]
    fn delimit_by_nul() {
        assert_eq!(delimited(b"a\0b\0", &[0]), [b"a\0", b"b\0"]);
    }

    // Every chunk the Split produces, in order — the materialized chunking spec.
    fn chunks<S: Split>(split: S, input: &[u8]) -> Vec<Vec<u8>> {
        let mut input = BufReader::new(input);
        let mut chunks = Vec::new();
        loop {
            let mut buf = Vec::new();
            if split.read(&mut input, &mut buf).unwrap() == 0 {
                return chunks;
            }
            chunks.push(buf);
        }
    }

    // Mirrors the production `&[b] / multi` delimiter-shape dispatch so the
    // boundary pins below stay per-kind.
    fn delimited(input: &[u8], delimiter: &[u8]) -> Vec<Vec<u8>> {
        match delimiter {
            &[b] => chunks(Byte(b), input),
            multi => chunks(Bytes(multi), input),
        }
    }

    // Reference: the chunks `IteratorExt::slice` selects, concatenated.
    // `slice_window` (step None) and `slice_stepped` must reproduce this
    // byte-for-byte.
    fn reference<S: Split>(
        split: S,
        input: &[u8],
        start: usize,
        end: Option<usize>,
        step: Option<NonZeroUsize>,
    ) -> Vec<u8> {
        chunks(split, input)
            .into_iter()
            .slice(start, end, step)
            .collect::<Vec<_>>()
            .concat()
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
        (0, None),
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
                reference(split, input, start, end, None),
                "slice_window diverged from the iterator for {start}:{end:?} on {input:?}"
            );
        }
    }

    const STEPS: &[usize] = &[1, 2, 3, 7];

    fn stepped<S: Split>(
        split: S,
        input: &[u8],
        start: usize,
        end: Option<usize>,
        step: NonZeroUsize,
        capacity: usize,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        slice_stepped(
            split,
            BufReader::with_capacity(capacity, input),
            &mut out,
            start,
            end,
            step,
        )
        .unwrap();
        out
    }

    fn assert_stepped_parity<S: Split + Copy>(split: S, input: &[u8]) {
        for &(start, end) in RANGES {
            for &step in STEPS {
                let step = NonZeroUsize::new(step).unwrap();
                assert_eq!(
                    stepped(split, input, start, end, step, 8 * 1024),
                    reference(split, input, start, end, Some(step)),
                    "slice_stepped diverged from the iterator for {start}:{end:?}:{step} on {input:?}"
                );
            }
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
    fn stepped_parity_line() {
        assert_stepped_parity(ByteRef(b'\n'), b"a\nb\nc\nd\ne\n");
        assert_stepped_parity(ByteRef(b'\n'), b"a\nb\nc"); // no trailing newline
    }

    #[test]
    fn stepped_parity_comma() {
        assert_stepped_parity(ByteRef(b','), b"a,b,c,d,e,");
    }

    #[test]
    fn stepped_parity_nul() {
        assert_stepped_parity(ByteRef(0), b"a\0b\0c\0d\0");
    }

    #[test]
    fn stepped_parity_multibyte() {
        assert_stepped_parity(BytesRef(b"||"), b"a||b||c||d||");
        assert_stepped_parity(BytesRef(b"||"), b"a||b||c"); // no trailing delimiter
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
                    reference(BytesRef(delim), input, start, end, None),
                    "straddle diverged for {start}:{end:?} on {input:?} delim {delim:?}"
                );
            }
        }
    }

    #[test]
    fn stepped_straddle() {
        // capacity 1 forces every delimiter to straddle a fill_buf boundary;
        // stepping interleaves Bytes::skip's rolling tail with Bytes::read's
        // buffered ends_with check on alternating chunks.
        for (input, delim) in [
            (&b"a||b||c||d||e"[..], &b"||"[..]),
            (&b"a|||b|"[..], &b"||"[..]),
            (&b"aaaaaa"[..], &b"aaa"[..]),
            (&b"aaaa"[..], &b"aaa"[..]),
        ] {
            for &(start, end) in RANGES {
                for &step in STEPS {
                    let step = NonZeroUsize::new(step).unwrap();
                    assert_eq!(
                        stepped(BytesRef(delim), input, start, end, step, 1),
                        reference(BytesRef(delim), input, start, end, Some(step)),
                        "stepped straddle diverged for {start}:{end:?}:{step} on {input:?} delim {delim:?}"
                    );
                }
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
