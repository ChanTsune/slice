//! User-perceived character elements (UAX #29 extended grapheme clusters).
//! Within each maximal valid UTF-8 run, elements are the run's grapheme
//! clusters; a byte that starts no valid sequence is one element of its own
//! and terminates the segmentation context, so clusters never span invalid
//! bytes and any byte stream splits without failing. Selected elements are
//! emitted verbatim (output is a byte subsequence of the input).

use crate::ext::{
    buf_read::Split,
    utf8::{Elem, Scanner},
};
use std::io::{self, BufRead, Write};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// The element slices of in-memory data; the reverse path walks these, and
/// the streaming kind is tested against them.
pub(crate) struct GraphemeElements<'a> {
    data: &'a [u8],
    /// Clusters of the current valid run; `data` is the unsegmented
    /// remainder after it.
    run: unicode_segmentation::Graphemes<'a>,
}

impl<'a> GraphemeElements<'a> {
    #[inline]
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            run: "".graphemes(true),
        }
    }
}

impl<'a> Iterator for GraphemeElements<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if let Some(cluster) = self.run.next() {
            return Some(cluster.as_bytes());
        }
        if self.data.is_empty() {
            return None;
        }
        let valid = match std::str::from_utf8(self.data) {
            Ok(s) => s,
            Err(err) if err.valid_up_to() > 0 => {
                // SAFETY: from_utf8 validated exactly this prefix.
                unsafe { std::str::from_utf8_unchecked(&self.data[..err.valid_up_to()]) }
            }
            // An invalid sequence or a truncated one at the end of the data:
            // the element is the single byte at the head, as in chars mode.
            Err(_) => {
                let (element, rest) = self.data.split_at(1);
                self.data = rest;
                return Some(element);
            }
        };
        self.data = &self.data[valid.len()..];
        self.run = valid.graphemes(true);
        self.run.next().map(str::as_bytes)
    }
}

/// A scalar element from the chars scanner is a valid character unless it is
/// a single non-ASCII byte — multi-byte elements are validated sequences.
#[inline]
fn is_valid_char(element: &[u8]) -> bool {
    element.len() > 1 || element[0] < 0x80
}

/// Whether `candidate` (one valid scalar) continues the cluster; on join it
/// is appended to the buffer. The buffer is exactly the text since the last
/// boundary, which is sufficient left context for every UAX #29 rule: RI
/// parity works because boundaries only fall on even regional-indicator
/// counts (cluster-local parity equals global parity), and the GB9c/GB11
/// lookback never crosses a boundary. A fresh cursor per check keeps the
/// chunk protocol trivial (whole-string chunk, so context is always
/// complete); the lookback cost is bounded by the cluster length, which only
/// degenerate inputs grow.
fn joins_cluster(cluster: &mut Vec<u8>, candidate: &[u8]) -> bool {
    let boundary_at = cluster.len();
    cluster.extend_from_slice(candidate);
    // SAFETY: only validated scalar elements reach the cluster buffer.
    let text = unsafe { std::str::from_utf8_unchecked(cluster) };
    let mut cursor = GraphemeCursor::new(boundary_at, text.len(), true);
    let boundary = cursor
        .is_boundary(text, 0)
        .expect("the chunk spans the whole string, so context is complete");
    if boundary {
        cluster.truncate(boundary_at);
    }
    !boundary
}

/// The element read past the current cluster's end.
enum Carried {
    /// A valid scalar: it seeds the next cluster.
    Char(Elem),
    /// An invalid byte: it is the next element by itself.
    Invalid(Elem),
}

/// The streaming counterpart of [`GraphemeElements`]: each chunk is one
/// element. Layered on the chars [`Scanner`] (which owns the fill_buf
/// straddle handling); this layer groups scalars into clusters, holding the
/// one element read past each boundary in `carry` and the current cluster's
/// bytes as segmentation context.
pub(crate) struct Graphemes {
    scanner: Scanner,
    carry: Option<Carried>,
    cluster: Vec<u8>,
}

impl Graphemes {
    #[inline]
    pub(crate) fn new() -> Self {
        Self {
            scanner: Scanner::new(),
            carry: None,
            cluster: Vec::new(),
        }
    }

    /// Resolve and consume one element, feeding its bytes to `sink`
    /// incrementally (scalar by scalar, so a record limit fails early).
    /// Returns the element's byte length; `Ok(0)` only at true end of stream.
    fn next_cluster<R: BufRead + ?Sized>(
        &mut self,
        r: &mut R,
        mut sink: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<usize> {
        let Self {
            scanner,
            carry,
            cluster,
        } = self;
        cluster.clear();
        match carry.take() {
            Some(Carried::Invalid(element)) => {
                sink(element.as_slice())?;
                return Ok(element.as_slice().len());
            }
            Some(Carried::Char(element)) => {
                sink(element.as_slice())?;
                cluster.extend_from_slice(element.as_slice());
            }
            None => {}
        }
        // One scalar at a time via next_element: its 4-byte probe keeps the
        // cost O(1) per character, where a block-validating walk restarted
        // here per cluster would re-scan the block tail every time and turn
        // quadratic.
        loop {
            let mut scalar = Elem::new(&[]);
            let len = scanner.next_element(r, |bytes| {
                scalar = Elem::new(bytes);
                Ok(())
            })?;
            if len == 0 {
                // EOF ends the cluster; Ok(0) only when nothing was open.
                return Ok(cluster.len());
            }
            let bytes = scalar.as_slice();
            if is_valid_char(bytes) {
                if cluster.is_empty() {
                    sink(bytes)?;
                    cluster.extend_from_slice(bytes);
                } else if joins_cluster(cluster, bytes) {
                    sink(bytes)?;
                } else {
                    *carry = Some(Carried::Char(scalar));
                    return Ok(cluster.len());
                }
            } else if cluster.is_empty() {
                // Nothing to terminate: the invalid byte is the element.
                sink(bytes)?;
                return Ok(len);
            } else {
                *carry = Some(Carried::Invalid(scalar));
                return Ok(cluster.len());
            }
        }
    }
}

impl Split for Graphemes {
    #[inline]
    fn read_to<R: BufRead + ?Sized, W: Write + ?Sized>(
        &mut self,
        r: &mut R,
        w: &mut W,
    ) -> io::Result<usize> {
        self.next_cluster(r, |bytes| w.write_all(bytes))
    }

    #[inline]
    fn skip<R: BufRead + ?Sized>(&mut self, r: &mut R) -> io::Result<usize> {
        self.next_cluster(r, |_| Ok(()))
    }

    /// The unbounded tail must include the carried element and the scanner's
    /// straddle bytes before handing the reader to `io::copy`. `cluster` is
    /// never part of the tail: its bytes belong to the element last resolved,
    /// already written by `read_to` or deliberately dropped by `skip`.
    fn copy_rest<R: BufRead + ?Sized, W: Write + ?Sized>(
        &mut self,
        r: &mut R,
        w: &mut W,
    ) -> io::Result<u64> {
        let mut flushed = 0;
        if let Some(Carried::Char(element) | Carried::Invalid(element)) = self.carry.take() {
            w.write_all(element.as_slice())?;
            flushed = element.as_slice().len() as u64;
        }
        Ok(flushed + self.scanner.copy_rest(r, w)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elements(data: &[u8]) -> Vec<&[u8]> {
        GraphemeElements::new(data).collect()
    }

    #[test]
    fn empty() {
        assert_eq!(elements(b""), Vec::<&[u8]>::new());
    }

    #[test]
    fn ascii() {
        assert_eq!(elements(b"abc"), vec![b"a", b"b", b"c"]);
    }

    #[test]
    fn combining_mark_joins_its_base() {
        // e + U+0301 COMBINING ACUTE ACCENT is one user-perceived character.
        assert_eq!(elements("e\u{301}x".as_bytes()).len(), 2);
        assert_eq!(elements("e\u{301}x".as_bytes())[0], "e\u{301}".as_bytes());
    }

    #[test]
    fn zwj_family_is_one_element() {
        assert_eq!(elements("👨‍👩‍👧".as_bytes()), vec!["👨‍👩‍👧".as_bytes()]);
    }

    #[test]
    fn skin_tone_modifier_joins() {
        assert_eq!(elements("👍🏽!".as_bytes()).len(), 2);
    }

    #[test]
    fn regional_indicator_pairs_split_per_flag() {
        // Four regional indicators form exactly two flags.
        assert_eq!(
            elements("🇯🇵🇺🇸".as_bytes()),
            vec!["🇯🇵".as_bytes(), "🇺🇸".as_bytes()]
        );
        assert_eq!(elements("🇯🇵🇯🇵".as_bytes()).len(), 2);
    }

    #[test]
    fn crlf_is_one_element() {
        assert_eq!(
            elements(b"a\r\nb"),
            vec![&b"a"[..], &b"\r\n"[..], &b"b"[..]]
        );
    }

    #[test]
    fn valid_text_matches_the_crate_segmentation() {
        let text = "日本語 with e\u{301} and 👨‍👩‍👧🇯🇵 and क्षि plus a\r\nb";
        let by_cluster: Vec<&[u8]> = text.graphemes(true).map(str::as_bytes).collect();
        assert_eq!(elements(text.as_bytes()), by_cluster);
    }

    #[test]
    fn invalid_byte_is_its_own_element_and_breaks_the_cluster() {
        // The combining mark cannot reach back across the invalid byte, so it
        // forms a degenerate cluster of its own.
        let mut data = b"e".to_vec();
        data.push(0xFF);
        data.extend_from_slice("\u{301}".as_bytes());
        assert_eq!(
            elements(&data),
            vec![&b"e"[..], &[0xFF][..], "\u{301}".as_bytes()]
        );
    }

    #[test]
    fn invalid_byte_resyncs_to_a_following_cluster() {
        let mut data = vec![0xFF];
        data.extend_from_slice("e\u{301}".as_bytes());
        assert_eq!(elements(&data), vec![&[0xFF][..], "e\u{301}".as_bytes()]);
    }

    #[test]
    fn truncated_sequence_at_end_splits_per_byte() {
        // E3 81 are the first two bytes of あ.
        assert_eq!(elements(&[0xE3, 0x81]), vec![&[0xE3][..], &[0x81][..]]);
    }

    #[test]
    fn elements_concatenate_back_to_the_input() {
        let mut data = "aあ👨‍👩‍👧".as_bytes().to_vec();
        data.extend_from_slice(&[0xFF, 0x80]);
        data.extend_from_slice("e\u{301}🇯🇵\r\n".as_bytes());
        data.extend_from_slice(&[0xF0, 0x90]);
        let joined: Vec<u8> = GraphemeElements::new(&data).flatten().copied().collect();
        assert_eq!(joined, data);
    }

    mod streaming {
        use super::*;
        use crate::ext::buf_read::Split;
        use std::io::BufReader;

        const CAPACITIES: [usize; 6] = [1, 2, 3, 4, 5, 8192];

        // Clusters of every stripe (ZWJ, flags, combining, CRLF), invalid
        // bytes between valid runs, and a truncated sequence at EOF; small
        // capacities straddle every boundary kind, including mid-cluster.
        fn mixed_data() -> Vec<u8> {
            let mut data = "aあ👨‍👩‍👧🇯🇵🇺🇸e\u{301}".as_bytes().to_vec();
            data.extend_from_slice(&[0xFF, 0x80]);
            data.extend_from_slice("👍🏽\r\nx".as_bytes());
            data.extend_from_slice(&[0xF0, 0x90]);
            data
        }

        fn streamed(data: &[u8], capacity: usize) -> Vec<Vec<u8>> {
            let mut reader = BufReader::with_capacity(capacity, data);
            let mut split = Graphemes::new();
            let mut out = Vec::new();
            loop {
                let mut element = Vec::new();
                let n = split.read_to(&mut reader, &mut element).unwrap();
                if n == 0 {
                    assert!(element.is_empty(), "Ok(0) must not produce bytes");
                    break;
                }
                assert_eq!(n, element.len(), "length must match emitted bytes");
                out.push(element);
            }
            out
        }

        #[test]
        fn read_to_matches_in_memory_at_every_capacity() {
            let data = mixed_data();
            let expected: Vec<Vec<u8>> = GraphemeElements::new(&data).map(<[u8]>::to_vec).collect();
            for capacity in CAPACITIES {
                assert_eq!(streamed(&data, capacity), expected, "capacity {capacity}");
            }
        }

        #[test]
        fn skip_advances_exactly_like_read_to() {
            let data = mixed_data();
            for capacity in [1, 3, 8192] {
                let mut reading = BufReader::with_capacity(capacity, data.as_slice());
                let mut skipping = BufReader::with_capacity(capacity, data.as_slice());
                let mut reader = Graphemes::new();
                let mut skipper = Graphemes::new();
                loop {
                    let mut element = Vec::new();
                    let read = reader.read_to(&mut reading, &mut element).unwrap();
                    assert_eq!(read, skipper.skip(&mut skipping).unwrap());
                    if read == 0 {
                        break;
                    }
                }
            }
        }

        #[test]
        fn record_limit_bounds_a_degenerate_cluster() {
            use crate::ext::{
                read_all_with_record_limit, slice_tail, slice_tail_with_record_limit,
            };
            use std::num::NonZeroUsize;

            // One pathological ~20 KB cluster: a base with 10 000 combining
            // marks. The limit must fail it early; without a limit the whole
            // cluster is the last element.
            let mut data = b"a".to_vec();
            for _ in 0..10_000 {
                data.extend_from_slice("\u{301}".as_bytes());
            }

            let err = slice_tail_with_record_limit(
                Graphemes::new(),
                data.as_slice(),
                &mut Vec::new(),
                NonZeroUsize::MIN,
                None,
                NonZeroUsize::MIN,
                Some(1024),
            )
            .expect_err("a cluster past the limit must fail");
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
            assert!(
                err.to_string().contains("grapheme"),
                "the limit error must name grapheme mode: {err}"
            );

            let err = read_all_with_record_limit(Graphemes::new(), data.as_slice(), Some(1024))
                .expect_err("the reverse read is bounded the same way");
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

            let mut out = Vec::new();
            slice_tail(
                Graphemes::new(),
                data.as_slice(),
                &mut out,
                NonZeroUsize::MIN,
                None,
                NonZeroUsize::MIN,
            )
            .expect("unlimited keeps working");
            assert_eq!(out, data);
        }

        #[test]
        fn skip_n_then_copy_rest_round_trips() {
            let data = mixed_data();
            let total = GraphemeElements::new(&data).count();
            for n in 0..=total + 2 {
                for capacity in [1, 3, 8192] {
                    let mut reader = BufReader::with_capacity(capacity, data.as_slice());
                    let mut split = Graphemes::new();
                    assert_eq!(
                        split.skip_n(&mut reader, n).unwrap(),
                        n.min(total),
                        "n={n} capacity={capacity}"
                    );
                    let mut rest = Vec::new();
                    split.copy_rest(&mut reader, &mut rest).unwrap();
                    let expected: Vec<u8> = GraphemeElements::new(&data)
                        .skip(n)
                        .flatten()
                        .copied()
                        .collect();
                    assert_eq!(rest, expected, "n={n} capacity={capacity}");
                }
            }
        }
    }
}
