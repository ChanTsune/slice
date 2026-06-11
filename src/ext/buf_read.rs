use crate::ext::IteratorExt;
use memchr::memmem;
use std::{
    io::{self, BufRead, Write},
    num::NonZeroUsize,
};

/// Scan to the first `delim` byte. `sink` receives each consumed slice; it is the
/// only thing that differs between emitting (write through) and skipping
/// (discard), and a sink error aborts the scan. Returns the total bytes
/// consumed: `Ok(0)` means the stream was already at EOF (no chunk),
/// `Ok(n > 0)` means a chunk was consumed — including a final chunk that lacks
/// a trailing `delim`.
#[inline]
fn scan_until<R: BufRead + ?Sized>(
    r: &mut R,
    delim: u8,
    mut sink: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<usize> {
    let mut read = 0;
    loop {
        let (done, used) = {
            let available = match r.fill_buf() {
                Ok([]) => return Ok(read),
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            match memchr::memchr(delim, available) {
                Some(i) => {
                    sink(&available[..=i])?;
                    (true, i + 1)
                }
                None => {
                    sink(available)?;
                    (false, available.len())
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

#[inline]
fn skip_until<R: BufRead + ?Sized>(r: &mut R, delim: u8) -> io::Result<usize> {
    scan_until(r, delim, |_| Ok(()))
}

/// A strategy for cutting a stream into chunks. Each chunk keeps its trailing
/// delimiter; the last chunk may lack one. `read_to` and `skip` are symmetric:
/// both advance the reader by exactly one chunk and return its byte length
/// (`Ok(0)` only at end of stream); `read_to` additionally writes the chunk to
/// `w`, straight from the reader's buffer. Kinds are separate types so the
/// single-byte path never pays for the multi-byte straddle machinery.
pub(crate) trait Split {
    fn read_to<R: BufRead + ?Sized, W: Write + ?Sized>(
        &self,
        r: &mut R,
        w: &mut W,
    ) -> io::Result<usize>;
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize>;

    /// Skip up to `n` chunks, returning how many were skipped; fewer than `n`
    /// means end of stream, with a terminal delimiter-less fragment counting
    /// as one chunk. Equivalent to `n` `skip` calls; kinds with a cheaper bulk
    /// scan override it.
    #[inline]
    fn skip_n<R: BufRead + ?Sized>(&self, r: &mut R, n: usize) -> io::Result<usize> {
        for skipped in 0..n {
            if self.skip(r)? == 0 {
                return Ok(skipped);
            }
        }
        Ok(n)
    }
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
    fn read_to<R: BufRead + ?Sized, W: Write + ?Sized>(
        &self,
        r: &mut R,
        w: &mut W,
    ) -> io::Result<usize> {
        scan_until(r, self.0, |chunk| w.write_all(chunk))
    }
    #[inline]
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
        skip_until(r, self.0)
    }

    /// Counts delimiters per `fill_buf` block instead of re-entering the
    /// scanner once per chunk, so skipping millions of short chunks costs one
    /// `memchr_iter` pass per block.
    fn skip_n<R: BufRead + ?Sized>(&self, r: &mut R, n: usize) -> io::Result<usize> {
        // Not just a fast path: the loop stops on `skipped + found == n`, which
        // n = 0 can never satisfy once a delimiter is counted.
        if n == 0 {
            return Ok(0);
        }
        let mut skipped = 0;
        // Whether bytes were consumed after the most recent delimiter: at end
        // of stream such a trailing fragment counts as one chunk, like `skip`.
        let mut fragment = false;
        loop {
            let (used, found) = {
                let block = match r.fill_buf() {
                    Ok([]) => return Ok(skipped + fragment as usize),
                    Ok(b) => b,
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                };
                let mut found = 0;
                let mut last_end = 0;
                for i in memchr::memchr_iter(self.0, block) {
                    found += 1;
                    last_end = i + 1;
                    if skipped + found == n {
                        break;
                    }
                }
                if skipped + found == n {
                    (last_end, found)
                } else {
                    fragment = last_end < block.len();
                    (block.len(), found)
                }
            };
            r.consume(used);
            skipped += found;
            if skipped == n {
                return Ok(n);
            }
        }
    }
}

impl Split for Bytes<'_> {
    #[inline]
    fn read_to<R: BufRead + ?Sized, W: Write + ?Sized>(
        &self,
        r: &mut R,
        w: &mut W,
    ) -> io::Result<usize> {
        scan_until_delim(r, self.delimiter, &self.finder, |chunk| w.write_all(chunk))
    }

    #[inline]
    fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
        scan_until_delim(r, self.delimiter, &self.finder, |_| Ok(()))
    }
}

/// Multi-byte counterpart of `scan_until`: scan to the first full `delimiter`
/// match, same contract (`sink` receives each consumed slice and may abort with
/// an error, `Ok(0)` means EOF, the final chunk may lack the delimiter).
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
    mut sink: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<usize> {
    let keep = delimiter.len() - 1;
    // Allocated lazily: the carry is only written when a chunk spans a
    // fill_buf boundary, so non-straddling chunks stay allocation-free.
    let mut carry: Vec<u8> = Vec::new();
    let mut read = 0;
    loop {
        let (done, used) = {
            let block = match r.fill_buf() {
                Ok([]) => return Ok(read),
                Ok(n) => n,
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
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
                    sink(&block[..used])?;
                    (true, used)
                }
                None => {
                    sink(block)?;
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
/// writes `end - start` more chunks straight from the reader's buffer. `end`
/// is an absolute chunk index from the stream start, matching
/// IteratorExt::slice's take(end).skip(start) ordering, so start >= end yields
/// an empty window.
pub(crate) fn slice_window<S: Split, R: BufRead, W: Write>(
    split: S,
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
) -> io::Result<()> {
    if split.skip_n(&mut input, start)? < start {
        return output.flush();
    }
    match end {
        None => {
            io::copy(&mut input, &mut output)?;
        }
        Some(end) => {
            let count = end.saturating_sub(start);
            for _ in 0..count {
                if split.read_to(&mut input, &mut output)? == 0 {
                    break;
                }
            }
        }
    }
    output.flush()
}

/// Stepped (step > 1) line/delimiter path. The surviving chunk indices come
/// from running `IteratorExt::slice` over the index stream itself, so the
/// take(end).skip(start).step_by(step) ordering is inherited from the same
/// adapter as every other mode rather than re-derived. Survivors are written
/// straight from the reader's buffer; the gaps between them are skipped
/// without copying. Stops at end of stream or after the last selected index,
/// so a bounded `end` never reads past its final selected chunk.
pub(crate) fn slice_stepped<S: Split, R: BufRead, W: Write>(
    split: S,
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
    step: NonZeroUsize,
) -> io::Result<()> {
    let mut index = 0usize;
    for target in (0usize..).slice(start, end, Some(step)) {
        let gap = target - index;
        let skipped = split.skip_n(&mut input, gap)?;
        index += skipped;
        if skipped < gap {
            break;
        }
        if split.read_to(&mut input, &mut output)? == 0 {
            break;
        }
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
            if split.read_to(&mut input, &mut buf).unwrap() == 0 {
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
        fn read_to<R: BufRead + ?Sized, W: Write + ?Sized>(
            &self,
            r: &mut R,
            w: &mut W,
        ) -> io::Result<usize> {
            Byte(self.0).read_to(r, w)
        }
        fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
            Byte(self.0).skip(r)
        }
        // Forwarded so the parity harnesses exercise Byte's bulk override.
        fn skip_n<R: BufRead + ?Sized>(&self, r: &mut R, n: usize) -> io::Result<usize> {
            Byte(self.0).skip_n(r, n)
        }
    }

    #[derive(Clone, Copy)]
    struct BytesRef<'d>(&'d [u8]);
    impl Split for BytesRef<'_> {
        fn read_to<R: BufRead + ?Sized, W: Write + ?Sized>(
            &self,
            r: &mut R,
            w: &mut W,
        ) -> io::Result<usize> {
            Bytes::new(self.0).read_to(r, w)
        }
        fn skip<R: BufRead + ?Sized>(&self, r: &mut R) -> io::Result<usize> {
            Bytes::new(self.0).skip(r)
        }
    }

    // skip_n must match `n` one-by-one skips exactly: same chunk count and
    // same remaining stream. Small capacities force the delimiter to land as
    // the last byte of a block and fragments to span blocks.
    fn assert_skip_n_matches_skip<S: Split>(split: S, input: &[u8]) {
        use std::io::Read;
        for n in 0..=6 {
            for capacity in [1, 2, 3, 8 * 1024] {
                let mut bulk = BufReader::with_capacity(capacity, input);
                let bulk_count = split.skip_n(&mut bulk, n).unwrap();

                let mut manual = BufReader::with_capacity(capacity, input);
                let mut manual_count = 0;
                while manual_count < n && split.skip(&mut manual).unwrap() > 0 {
                    manual_count += 1;
                }

                assert_eq!(
                    bulk_count, manual_count,
                    "skip_n count diverged from skip for n={n} capacity={capacity} on {input:?}"
                );
                let mut bulk_rest = Vec::new();
                bulk.read_to_end(&mut bulk_rest).unwrap();
                let mut manual_rest = Vec::new();
                manual.read_to_end(&mut manual_rest).unwrap();
                assert_eq!(
                    bulk_rest, manual_rest,
                    "skip_n remainder diverged from skip for n={n} capacity={capacity} on {input:?}"
                );
            }
        }
    }

    #[test]
    fn skip_n_matches_skip_line() {
        for input in [
            &b""[..],
            b"a\nb\nc\nd\ne\n",
            b"a\nb\nc", // trailing fragment
            b"\n\n\n",
            b"abc",          // no delimiter: one fragment chunk
            b"abcd\nefgh\n", // chunks longer than the small capacities
            b"abcd\nefgh",
        ] {
            assert_skip_n_matches_skip(Byte(b'\n'), input);
        }
    }

    #[test]
    fn skip_n_matches_skip_nul() {
        for input in [&b"a\0b\0c\0d\0"[..], b"a\0b\0c", b"\0\0", b"ab"] {
            assert_skip_n_matches_skip(Byte(0), input);
        }
    }

    #[test]
    fn skip_n_matches_skip_multibyte_default() {
        for input in [&b"a||b||c||d||"[..], b"a||b||c", b"a|||b|", b"aaaa"] {
            assert_skip_n_matches_skip(Bytes::new(b"||"), input);
        }
    }

    #[test]
    fn skip_n_zero_consumes_nothing() {
        use std::io::Read;
        let mut input = BufReader::new(&b"a\nb\n"[..]);
        assert_eq!(Byte(b'\n').skip_n(&mut input, 0).unwrap(), 0);
        let mut rest = Vec::new();
        input.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"a\nb\n");
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
        // `a|||b|` splits leftmost into `a||`, `|b|`.
        assert_eq!(
            windowed(BytesRef(b"||"), b"a|||b|", 1, None, 8 * 1024),
            b"|b|"
        );
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

    // Fails on its second write call. Chunks go to the writer straight from
    // the reader's buffer, so with capacity 1 a single chunk spans several
    // writes and the failure lands mid-chunk.
    struct FailOnSecondWrite(usize);

    impl Write for FailOnSecondWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0 += 1;
            if self.0 >= 2 {
                Err(io::Error::other("second write failed"))
            } else {
                Ok(buf.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn slice_window_surfaces_write_errors_mid_chunk() {
        let err = slice_window(
            Byte(b'\n'),
            BufReader::with_capacity(1, &b"abc\n"[..]),
            FailOnSecondWrite(0),
            0,
            Some(1),
        )
        .expect_err("a write failure mid-chunk must surface from slice_window");
        assert_eq!(err.kind(), io::ErrorKind::Other);

        let err = slice_window(
            Bytes::new(b"||"),
            BufReader::with_capacity(1, &b"abc||"[..]),
            FailOnSecondWrite(0),
            0,
            Some(1),
        )
        .expect_err("a write failure mid-chunk must surface from slice_window");
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn slice_stepped_surfaces_write_errors_mid_chunk() {
        let err = slice_stepped(
            Byte(b'\n'),
            BufReader::with_capacity(1, &b"abc\nd\ne\n"[..]),
            FailOnSecondWrite(0),
            0,
            None,
            NonZeroUsize::new(2).unwrap(),
        )
        .expect_err("a write failure mid-chunk must surface from slice_stepped");
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }

    // read_to's return contract: every chunk reports its full byte length, and
    // Ok(0) appears only at end of stream — where it stays Ok(0).
    fn assert_read_to_eof_contract<S: Split>(split: S, input: &[u8]) {
        let mut r = BufReader::with_capacity(2, input);
        let mut consumed = 0;
        loop {
            let mut buf = Vec::new();
            let n = split.read_to(&mut r, &mut buf).unwrap();
            if n == 0 {
                assert!(buf.is_empty(), "Ok(0) must not produce bytes");
                break;
            }
            assert_eq!(n, buf.len(), "consumed count must match emitted bytes");
            consumed += n;
        }
        assert_eq!(consumed, input.len(), "Ok(0) before the stream ran out");
        let mut buf = Vec::new();
        assert_eq!(split.read_to(&mut r, &mut buf).unwrap(), 0);
    }

    #[test]
    fn read_to_returns_zero_only_at_eof() {
        for input in [&b""[..], b"a\nb\nc\n", b"a\nb\nc", b"abc"] {
            assert_read_to_eof_contract(Byte(b'\n'), input);
        }
        for input in [&b""[..], b"a||b||c||", b"a||b||c", b"abc", b"|"] {
            assert_read_to_eof_contract(Bytes::new(b"||"), input);
        }
    }
}
