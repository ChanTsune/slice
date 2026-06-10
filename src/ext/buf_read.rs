use crate::ext::IteratorExt;
use memchr::memmem;
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

/// Multi-byte delimiter (len >= 2; constructed only by the delimiter-shape
/// dispatch). The `memmem` finder is precomputed here so its searcher setup is
/// paid once per run, not once per chunk.
pub(crate) struct Bytes<'d> {
    delimiter: &'d [u8],
    finder: memmem::Finder<'d>,
}

impl<'d> Bytes<'d> {
    #[inline]
    pub(crate) fn new(delimiter: &'d [u8]) -> Self {
        debug_assert!(
            delimiter.len() >= 2,
            "single-byte and empty delimiters dispatch to other kinds"
        );
        Self {
            delimiter,
            finder: memmem::Finder::new(delimiter),
        }
    }
}

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
        scan_until_delim(r, self.delimiter, &self.finder, |chunk| {
            buf.extend_from_slice(chunk)
        })
    }

    #[inline]
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
        scan_until_delim(r, self.delimiter, &self.finder, |_| {})
    }
}

/// Multi-byte counterpart of `scan_until`: scan to the first full `delimiter`
/// match, same contract (`sink` receives each consumed slice, `Ok(0)` means
/// EOF, the final chunk may lack the delimiter).
///
/// A match can straddle a `fill_buf` boundary, so the last `delimiter.len() - 1`
/// consumed bytes are carried across blocks. Each block is searched in two
/// stages, straddle first: the probe (`carry` ++ the block's first
/// `delimiter.len() - 1` bytes) is too short to hold a match starting inside
/// the block, so any probe hit starts in the carry and precedes every in-block
/// hit — the leftmost match overall wins. The carry lives for one chunk only,
/// so a confirmed match never seeds the next chunk's window (`aaaaaa` with
/// `aaa` -> `aaa`, `aaa`).
fn scan_until_delim<R: BufRead + ?Sized>(
    r: &mut R,
    delimiter: &[u8],
    finder: &memmem::Finder<'_>,
    mut sink: impl FnMut(&[u8]),
) -> io::Result<usize> {
    let keep = delimiter.len() - 1;
    // Allocated lazily: the carry is only written when a chunk spans a
    // fill_buf boundary, so non-straddling chunks stay allocation-free.
    let mut carry: Vec<u8> = Vec::new();
    let mut read = 0;
    loop {
        let (done, used) = {
            let block = match r.fill_buf() {
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if block.is_empty() {
                return Ok(read);
            }
            let straddle = if carry.is_empty() {
                None
            } else {
                let carried = carry.len();
                carry.extend_from_slice(&block[..block.len().min(keep)]);
                let hit = finder.find(&carry).map(|p| p + delimiter.len() - carried);
                carry.truncate(carried);
                hit
            };
            match straddle.or_else(|| finder.find(block).map(|i| i + delimiter.len())) {
                Some(used) => {
                    sink(&block[..used]);
                    (true, used)
                }
                None => {
                    sink(block);
                    extend_carry(&mut carry, block, keep);
                    (false, block.len())
                }
            }
        };
        r.consume(used);
        read += used;
        if done {
            return Ok(read);
        }
    }
}

/// Append `block` to `carry`, keeping only the trailing `keep` bytes (the most
/// a future delimiter match could need from before the current scan position).
#[inline]
fn extend_carry(carry: &mut Vec<u8>, block: &[u8], keep: usize) {
    if block.len() >= keep {
        carry.clear();
        carry.extend_from_slice(&block[block.len() - keep..]);
    } else {
        let overflow = (carry.len() + block.len()).saturating_sub(keep);
        carry.drain(..overflow);
        carry.extend_from_slice(block);
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
        chunks_at(split, input, 8 * 1024)
    }

    fn chunks_at<S: Split>(split: S, input: &[u8], capacity: usize) -> Vec<Vec<u8>> {
        let mut input = BufReader::with_capacity(capacity, input);
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
            multi => chunks(Bytes::new(multi), input),
        }
    }

    // Independent oracle: leftmost non-overlapping chunking of the in-memory
    // slice. Each chunk keeps its delimiter; the last may lack one.
    fn naive_chunks(input: &[u8], delimiter: &[u8]) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        let mut rest = input;
        while !rest.is_empty() {
            match rest.windows(delimiter.len()).position(|w| w == delimiter) {
                Some(i) => {
                    let (chunk, tail) = rest.split_at(i + delimiter.len());
                    chunks.push(chunk.to_vec());
                    rest = tail;
                }
                None => {
                    chunks.push(rest.to_vec());
                    break;
                }
            }
        }
        chunks
    }

    // Overlapping-prefix needles (xyx, aaa), partial trailing delimiters,
    // delimiter at the very start, input == delimiter, input shorter than the
    // delimiter, empty input.
    const ORACLE_CASES: &[(&[u8], &[u8])] = &[
        (b"a|b|", b"||"),
        (b"a|||b|", b"||"),
        (b"aaaaaa", b"aaa"),
        (b"aaaa", b"aaa"),
        (b"aaaaa", b"aaa"),
        (b"abcabc", b"abc"),
        (b"xyxyxy", b"xyx"),
        (b"a||b||c", b"||"),
        (b"a||b|", b"||"),
        (b"||a||", b"||"),
        (b"||", b"||"),
        (b"|", b"||"),
        (b"", b"||"),
        (b"\xaa\xff\xbb\xaa\xff\xcc", b"\xaa\xff"),
        (b"xabcdabcdx", b"abcd"),
    ];

    #[test]
    fn multibyte_chunks_match_naive_oracle() {
        for &(input, delim) in ORACLE_CASES {
            assert_eq!(
                chunks(Bytes::new(delim), input),
                naive_chunks(input, delim),
                "chunks diverged from the naive oracle on {input:?} delim {delim:?}"
            );
        }
    }

    #[test]
    fn multibyte_chunks_match_naive_oracle_across_block_boundaries() {
        // Capacities 1..=3 force every match to straddle fill_buf boundaries.
        for &(input, delim) in ORACLE_CASES {
            for capacity in 1..=3 {
                assert_eq!(
                    chunks_at(Bytes::new(delim), input, capacity),
                    naive_chunks(input, delim),
                    "chunks diverged from the naive oracle on {input:?} delim {delim:?} capacity {capacity}"
                );
            }
        }
    }

    #[test]
    fn multibyte_skip_matches_naive_oracle_across_block_boundaries() {
        // Skipping the first chunk must leave exactly the oracle's remainder,
        // pinning skip's consumed byte count independently of read.
        for &(input, delim) in ORACLE_CASES {
            let expected: Vec<u8> = naive_chunks(input, delim)
                .into_iter()
                .skip(1)
                .flatten()
                .collect();
            for capacity in 1..=3 {
                assert_eq!(
                    windowed(BytesRef(delim), input, 1, None, capacity),
                    expected,
                    "skip diverged from the naive oracle on {input:?} delim {delim:?} capacity {capacity}"
                );
            }
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
            Bytes::new(self.0).read(r, buf)
        }
        fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
            Bytes::new(self.0).skip(r)
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
    fn multibyte_overlap() {
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
    fn multibyte_contiguous_repeat() {
        // A confirmed match must not seed the next chunk's match window:
        // `aaaaaa` splits into `aaa`,`aaa`, so skipping one chunk leaves `aaa`.
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
    fn multibyte_straddle() {
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
        // stepping drives scan_until_delim's carry from both the skip and the
        // read call sites on alternating chunks.
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
}
