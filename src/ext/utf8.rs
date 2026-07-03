//! UTF-8 character elements. An element is the valid UTF-8 sequence starting
//! at the current position (1-4 bytes), or that single byte when no valid
//! sequence starts there — so any byte stream splits without failing and the
//! selected elements round-trip verbatim. On valid text the elements are
//! exactly Python's `str` characters (Unicode scalar values).

use crate::{ext::IteratorExt, range::SliceIndex};
use std::{
    collections::VecDeque,
    io::{self, BufRead, Write},
    num::NonZeroUsize,
};

/// Element boundary decision at the head of `window` (at least 1 byte). With
/// 4 bytes of lookahead the decision is always final; `NeedMore` can only
/// occur for a shorter window, and callers map it to `Complete(1)` at end of
/// input (a truncated sequence splits into one element per byte).
pub(crate) enum Decision {
    Complete(usize),
    NeedMore,
}

pub(crate) fn element_len(window: &[u8]) -> Decision {
    debug_assert!(!window.is_empty());
    let probe = &window[..window.len().min(4)];
    match std::str::from_utf8(probe) {
        Ok(s) => Decision::Complete(first_char_len(s)),
        Err(err) if err.valid_up_to() > 0 => {
            // SAFETY: from_utf8 validated exactly this prefix.
            let valid = unsafe { std::str::from_utf8_unchecked(&probe[..err.valid_up_to()]) };
            Decision::Complete(first_char_len(valid))
        }
        // An invalid sequence: the element is the single byte at the head, so
        // a following valid sequence re-synchronizes at its own start.
        Err(err) if err.error_len().is_some() => Decision::Complete(1),
        Err(_) => {
            debug_assert!(probe.len() < 4, "4 bytes of lookahead always decide");
            Decision::NeedMore
        }
    }
}

#[inline]
fn first_char_len(s: &str) -> usize {
    s.chars()
        .next()
        .expect("validated nonempty prefix")
        .len_utf8()
}

/// The element slices of in-memory data; the reverse path walks these.
pub(crate) struct Utf8Elements<'a> {
    data: &'a [u8],
}

impl<'a> Utf8Elements<'a> {
    #[inline]
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl<'a> Iterator for Utf8Elements<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.data.is_empty() {
            return None;
        }
        let len = match element_len(self.data) {
            Decision::Complete(len) => len,
            // The data ends inside a sequence: one element per byte.
            Decision::NeedMore => 1,
        };
        let (element, rest) = self.data.split_at(len);
        self.data = rest;
        Some(element)
    }
}

/// One buffered element: a UTF-8 sequence or a single invalid byte, so at
/// most 4 bytes — callers hold these inline instead of allocating a heap
/// buffer per element.
#[derive(Clone, Copy)]
pub(super) struct Elem {
    bytes: [u8; 4],
    len: u8,
}

impl Elem {
    #[inline]
    pub(super) fn new(element: &[u8]) -> Self {
        let mut bytes = [0; 4];
        bytes[..element.len()].copy_from_slice(element);
        Self {
            bytes,
            len: element.len() as u8,
        }
    }

    #[inline]
    pub(super) fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

/// Append one element to the batch buffer, flushing at the write-buffer
/// granularity so sub-4-byte elements never reach the writer one by one.
#[inline]
fn write_batched<W: Write>(buf: &mut Vec<u8>, element: &[u8], output: &mut W) -> io::Result<()> {
    buf.extend_from_slice(element);
    if buf.len() >= crate::WRITE_BUF_SIZE {
        output.write_all(buf)?;
        buf.clear();
    }
    Ok(())
}

/// Streaming scanner behind the `char_*` drivers and the graphemes split. A
/// sequence can straddle a `fill_buf` boundary and `BufRead` cannot
/// un-consume, so bytes read past a block edge before the element was decided
/// wait in `pending` — e.g. a block ending in `F0 90` followed by `41` splits
/// into the elements `F0`, `90`, `41`, of which only `F0` belongs to the
/// element being resolved.
pub(super) struct Scanner {
    pending: [u8; 4],
    pending_len: u8,
}

/// How one turn of [`Scanner::next_element`]'s resolve loop advanced.
enum Step {
    /// An element was emitted: its length, and how much of it came from the
    /// current block (the rest came from `pending`).
    Element { len: usize, from_block: usize },
    /// The block ended inside a possibly-valid sequence; its bytes moved to
    /// `pending`, refill and retry.
    Stashed(usize),
}

impl Scanner {
    #[inline]
    pub(super) fn new() -> Self {
        Self {
            pending: [0; 4],
            pending_len: 0,
        }
    }

    #[inline]
    fn pending(&self) -> &[u8] {
        &self.pending[..self.pending_len as usize]
    }

    #[inline]
    fn push_pending(&mut self, bytes: &[u8]) {
        let len = self.pending_len as usize;
        debug_assert!(
            len + bytes.len() <= 4,
            "an undecided prefix is under 4 bytes"
        );
        self.pending[len..len + bytes.len()].copy_from_slice(bytes);
        self.pending_len = (len + bytes.len()) as u8;
    }

    #[inline]
    fn drop_pending(&mut self, n: usize) {
        let len = self.pending_len as usize;
        self.pending.copy_within(n..len, 0);
        self.pending_len = (len - n) as u8;
    }

    /// Resolve and consume one element, feeding its bytes to `sink` — at most
    /// one call, always the whole element (unlike the fragment-wise delimiter
    /// scanners); the graphemes split captures the element through an
    /// out-parameter closure and relies on this. Returns the element's byte
    /// length; `Ok(0)` only at true end of stream (EOF and nothing pending).
    /// The straddle-aware slow path: the block walks below fall back to it
    /// whenever `pending` is non-empty.
    pub(super) fn next_element<R: BufRead + ?Sized>(
        &mut self,
        r: &mut R,
        mut sink: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<usize> {
        loop {
            let carried = self.pending_len as usize;
            let step = {
                let block = match r.fill_buf() {
                    Ok(block) => block,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(err),
                };
                if carried == 0 {
                    if block.is_empty() {
                        return Ok(0);
                    }
                    match element_len(block) {
                        Decision::Complete(len) => {
                            sink(&block[..len])?;
                            Step::Element {
                                len,
                                from_block: len,
                            }
                        }
                        // The whole (short) block is an undecided prefix.
                        Decision::NeedMore => {
                            self.push_pending(block);
                            Step::Stashed(block.len())
                        }
                    }
                } else {
                    // The element under decision is always a prefix of
                    // pending ++ block, so probe those (up to 4) bytes.
                    let mut probe = [0u8; 4];
                    probe[..carried].copy_from_slice(&self.pending[..carried]);
                    let extra = block.len().min(4 - carried);
                    probe[carried..carried + extra].copy_from_slice(&block[..extra]);
                    match element_len(&probe[..carried + extra]) {
                        Decision::Complete(len) => {
                            sink(&probe[..len])?;
                            Step::Element {
                                len,
                                from_block: len.saturating_sub(carried),
                            }
                        }
                        // EOF inside a sequence: one element per byte.
                        Decision::NeedMore if block.is_empty() => {
                            sink(&probe[..1])?;
                            Step::Element {
                                len: 1,
                                from_block: 0,
                            }
                        }
                        Decision::NeedMore => {
                            self.push_pending(&block[..extra]);
                            Step::Stashed(extra)
                        }
                    }
                }
            };
            match step {
                Step::Element { len, from_block } => {
                    self.drop_pending(len.min(carried));
                    r.consume(from_block);
                    return Ok(len);
                }
                Step::Stashed(n) => r.consume(n),
            }
        }
    }

    /// Visit every element in stream order until `visit` returns `false` or
    /// the stream ends. Each block is bulk-validated once with `from_utf8`,
    /// so a visit costs no per-element re-decode.
    pub(super) fn for_each<R: BufRead + ?Sized>(
        &mut self,
        r: &mut R,
        mut visit: impl FnMut(&[u8]) -> io::Result<bool>,
    ) -> io::Result<()> {
        loop {
            // A straddled prefix resolves element-by-element first.
            if self.pending_len > 0 {
                let mut more = true;
                let len = self.next_element(r, |element| {
                    more = visit(element)?;
                    Ok(())
                })?;
                if len == 0 || !more {
                    return Ok(());
                }
                continue;
            }
            let (consumed, more) = {
                let block = match r.fill_buf() {
                    Ok([]) => return Ok(()),
                    Ok(block) => block,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(err),
                };
                let mut i = 0;
                let mut more = true;
                'block: while i < block.len() {
                    let rest = &block[i..];
                    let valid = match std::str::from_utf8(rest) {
                        Ok(s) => s,
                        Err(err) if err.valid_up_to() > 0 => {
                            // SAFETY: from_utf8 validated exactly this prefix.
                            unsafe { std::str::from_utf8_unchecked(&rest[..err.valid_up_to()]) }
                        }
                        Err(err) if err.error_len().is_some() => {
                            more = visit(&rest[..1])?;
                            i += 1;
                            if !more {
                                break 'block;
                            }
                            continue;
                        }
                        // Incomplete sequence at the block end (under 4
                        // bytes): stash it and resolve against the next
                        // block — or EOF — via the pending path above.
                        Err(_) => {
                            self.push_pending(rest);
                            i = block.len();
                            continue;
                        }
                    };
                    for (offset, ch) in valid.char_indices() {
                        let len = ch.len_utf8();
                        more = visit(&rest[offset..offset + len])?;
                        if !more {
                            i += offset + len;
                            break 'block;
                        }
                    }
                    i += valid.len();
                }
                (i, more)
            };
            r.consume(consumed);
            if !more {
                return Ok(());
            }
        }
    }

    /// Advance past up to `n` elements, feeding every consumed byte to `sink`
    /// in whole-block spans; returns the number advanced (fewer means end of
    /// stream). A discarding sink makes this a bulk skip, a writing sink a
    /// bulk copy.
    fn advance<R: BufRead + ?Sized>(
        &mut self,
        r: &mut R,
        n: usize,
        mut sink: impl FnMut(&[u8]) -> io::Result<()>,
    ) -> io::Result<usize> {
        let mut advanced = 0;
        while advanced < n {
            // A straddled prefix resolves element-by-element first.
            if self.pending_len > 0 {
                if self.next_element(r, &mut sink)? == 0 {
                    return Ok(advanced);
                }
                advanced += 1;
                continue;
            }
            let consumed = {
                let block = match r.fill_buf() {
                    Ok([]) => return Ok(advanced),
                    Ok(block) => block,
                    Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                    Err(err) => return Err(err),
                };
                let mut i = 0;
                let mut stashed = false;
                while i < block.len() && advanced < n {
                    let rest = &block[i..];
                    let valid = match std::str::from_utf8(rest) {
                        Ok(s) => s,
                        Err(err) if err.valid_up_to() > 0 => {
                            // SAFETY: from_utf8 validated exactly this prefix.
                            unsafe { std::str::from_utf8_unchecked(&rest[..err.valid_up_to()]) }
                        }
                        Err(err) if err.error_len().is_some() => {
                            i += 1;
                            advanced += 1;
                            continue;
                        }
                        // Incomplete sequence at the block end: its bytes are
                        // not yet attributed to an element, so they move to
                        // pending and stay out of the emitted span.
                        Err(_) => {
                            self.push_pending(rest);
                            stashed = true;
                            break;
                        }
                    };
                    // One manual walk both counts a short prefix and finds
                    // the stop offset.
                    let mut consumed = valid.len();
                    for (offset, _) in valid.char_indices() {
                        if advanced == n {
                            consumed = offset;
                            break;
                        }
                        advanced += 1;
                    }
                    i += consumed;
                }
                sink(&block[..i])?;
                if stashed {
                    block.len()
                } else {
                    i
                }
            };
            r.consume(consumed);
        }
        Ok(n)
    }

    /// Emit everything not yet consumed verbatim: any straddle-consumed bytes
    /// first, then the reader's remainder via `io::copy`.
    pub(super) fn copy_rest<R: BufRead + ?Sized, W: Write + ?Sized>(
        &mut self,
        r: &mut R,
        w: &mut W,
    ) -> io::Result<u64> {
        let carried = self.pending_len as u64;
        w.write_all(self.pending())?;
        self.pending_len = 0;
        Ok(carried + io::copy(r, w)?)
    }
}

/// Chars-mode unit-step window: skip `start` elements block-wise, then either
/// copy the rest verbatim (unbounded) or pass `end - start` elements through
/// as whole-block spans.
pub(crate) fn char_window<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
) -> io::Result<()> {
    let mut scanner = Scanner::new();
    if scanner.advance(&mut input, start, |_| Ok(()))? < start {
        return output.flush();
    }
    match end {
        None => {
            scanner.copy_rest(&mut input, &mut output)?;
        }
        Some(end) => {
            scanner.advance(&mut input, end.saturating_sub(start), |span| {
                output.write_all(span)
            })?;
        }
    }
    output.flush()
}

/// Chars-mode stepped path: emits the elements at indices i in
/// [start, min(end, len)) with (i - start) % step == 0, batching the selected
/// elements into write-buffer-sized runs.
pub(crate) fn char_stepped<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
    step: NonZeroUsize,
) -> io::Result<()> {
    let mut remaining = end.map(|end| end.saturating_sub(start));
    if remaining == Some(0) {
        return output.flush();
    }
    let mut scanner = Scanner::new();
    if scanner.advance(&mut input, start, |_| Ok(()))? < start {
        return output.flush();
    }
    let step = step.get();
    let mut buf = Vec::with_capacity(crate::WRITE_BUF_SIZE + 4);
    // Element phase within the stride; 0 selects.
    let mut phase = 0;
    scanner.for_each(&mut input, |element| {
        if phase == 0 {
            write_batched(&mut buf, element, &mut output)?;
        }
        phase = (phase + 1) % step;
        match &mut remaining {
            Some(remaining) => {
                *remaining -= 1;
                Ok(*remaining > 0)
            }
            None => Ok(true),
        }
    })?;
    if !buf.is_empty() {
        output.write_all(&buf)?;
    }
    output.flush()
}

/// Chars-mode tail-relative start (`-k:…`): the last k elements ride a ring
/// of inline 4-byte slots; EOF fixes the length and the resolve arithmetic
/// picks the survivors. An absolute `end` freezes the ring once reached — the
/// remainder is counted without storing.
pub(crate) fn char_tail<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    back: NonZeroUsize,
    end: Option<SliceIndex>,
    step: NonZeroUsize,
) -> io::Result<()> {
    let k = back.get();
    let bound = match end {
        Some(SliceIndex::FromStart(end)) => Some(end),
        _ => None,
    };
    let mut scanner = Scanner::new();
    // Grown lazily toward k so a huge `-k:` never preallocates past the
    // elements actually seen.
    let mut ring: Vec<Elem> = Vec::new();
    let mut total = 0usize;
    scanner.for_each(&mut input, |element| {
        let slot = total % k;
        if ring.len() <= slot {
            ring.push(Elem::new(element));
        } else {
            ring[slot] = Elem::new(element);
        }
        total += 1;
        // Past an absolute end no element can be selected: freeze the ring
        // and bulk-count the remainder.
        Ok(bound.is_none_or(|end| total < end))
    })?;
    if bound.is_some_and(|end| total >= end) {
        total += scanner.advance(&mut input, usize::MAX, |_| Ok(()))?;
    }
    let len = total as u64;
    let start = SliceIndex::FromEnd(back).resolve(len) as usize;
    let end = end.map_or(total, |end| end.resolve(len) as usize);
    let mut buf = Vec::with_capacity(crate::WRITE_BUF_SIZE + 4);
    for i in (0..total).slice(start, Some(end), Some(step)) {
        write_batched(&mut buf, ring[i % k].as_slice(), &mut output)?;
    }
    if !buf.is_empty() {
        output.write_all(&buf)?;
    }
    output.flush()
}

/// Chars-mode tail-relative end (`start:-m`): element i is selected iff
/// i < L - m, certain once element i + m has been read, so emission lags m
/// elements behind through a queue of inline 4-byte slots. Only
/// stride-selected elements carry payload.
pub(crate) fn char_lag<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    start: usize,
    back: NonZeroUsize,
    step: NonZeroUsize,
) -> io::Result<()> {
    let mut scanner = Scanner::new();
    if scanner.advance(&mut input, start, |_| Ok(()))? < start {
        return output.flush();
    }
    let m = back.get();
    let step = step.get();
    // (relative element index, payload), oldest first.
    let mut lagging: VecDeque<(usize, Elem)> = VecDeque::new();
    let mut next = 0usize;
    let mut buf = Vec::with_capacity(crate::WRITE_BUF_SIZE + 4);
    scanner.for_each(&mut input, |element| {
        if next % step == 0 {
            lagging.push_back((next, Elem::new(element)));
        }
        next += 1;
        // Element i survives iff i + m < next; checked after the increment so
        // the element proving the m-th successor exists has already landed.
        while lagging
            .front()
            .is_some_and(|&(i, _)| i.saturating_add(m) < next)
        {
            let (_, element) = lagging.pop_front().expect("front was just matched");
            write_batched(&mut buf, element.as_slice(), &mut output)?;
        }
        Ok(true)
    })?;
    // EOF: whatever still lags lies within the dropped tail.
    if !buf.is_empty() {
        output.write_all(&buf)?;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elements(data: &[u8]) -> Vec<&[u8]> {
        Utf8Elements::new(data).collect()
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
    fn multibyte_widths() {
        // 1-, 3-, 4-, and 2-byte characters.
        assert_eq!(
            elements("aあ🍣é".as_bytes()),
            vec![
                "a".as_bytes(),
                "あ".as_bytes(),
                "🍣".as_bytes(),
                "é".as_bytes()
            ]
        );
    }

    #[test]
    fn python_parity_on_valid_text() {
        let text = "日本語テキスト with ASCII and 🍣🍺 emoji";
        let by_char: Vec<Vec<u8>> = text.chars().map(|c| c.to_string().into_bytes()).collect();
        assert_eq!(elements(text.as_bytes()), by_char);
    }

    #[test]
    fn lone_continuation_bytes_are_single_elements() {
        assert_eq!(elements(&[0x80, 0x80]), vec![&[0x80][..], &[0x80][..]]);
    }

    #[test]
    fn invalid_lead_is_a_single_element() {
        assert_eq!(elements(&[0xFF, b'a']), vec![&[0xFF][..], &[b'a'][..]]);
    }

    #[test]
    fn truncated_sequence_at_end_splits_per_byte() {
        // E3 81 are the first two bytes of あ.
        assert_eq!(elements(&[0xE3, 0x81]), vec![&[0xE3][..], &[0x81][..]]);
    }

    #[test]
    fn invalid_byte_resyncs_to_a_following_valid_char() {
        let mut data = vec![0xFF];
        data.extend_from_slice("あ".as_bytes());
        assert_eq!(elements(&data), vec![&[0xFF][..], "あ".as_bytes()]);
    }

    #[test]
    fn overlong_encoding_splits_per_byte() {
        // C0 80 is the overlong encoding of NUL, invalid per RFC 3629.
        assert_eq!(elements(&[0xC0, 0x80]), vec![&[0xC0][..], &[0x80][..]]);
    }

    #[test]
    fn surrogate_half_splits_per_byte() {
        // ED A0 80 encodes U+D800, a surrogate, invalid in UTF-8.
        assert_eq!(
            elements(&[0xED, 0xA0, 0x80]),
            vec![&[0xED][..], &[0xA0][..], &[0x80][..]]
        );
    }

    mod drivers {
        use super::*;
        use crate::ext::IteratorExt;
        use crate::range::SliceIndex;
        use std::io::BufReader;
        use std::num::NonZeroUsize;

        const CAPACITIES: [usize; 5] = [1, 2, 3, 4, 8192];

        fn nz(n: usize) -> NonZeroUsize {
            NonZeroUsize::new(n).unwrap()
        }

        // Every element-width transition, invalid bytes between valid runs,
        // and a truncated sequence at EOF.
        fn datasets() -> Vec<Vec<u8>> {
            let mut mixed = Vec::new();
            mixed.extend_from_slice("aあ🍣".as_bytes());
            mixed.extend_from_slice(&[0xFF, 0x80]);
            mixed.extend_from_slice("é日x".as_bytes());
            mixed.extend_from_slice(&[0xF0, 0x90]);
            vec![
                Vec::new(),
                b"abcdefgh".to_vec(),
                "あいうえおかきくけこ".as_bytes().to_vec(),
                mixed,
                vec![0xFF; 5],
            ]
        }

        // The Python-semantics reference: slice the in-memory element
        // sequence with the same adapter the drivers must reproduce.
        fn expected(data: &[u8], start: usize, end: Option<usize>, step: usize) -> Vec<u8> {
            Utf8Elements::new(data)
                .slice(start, end, Some(nz(step)))
                .flatten()
                .copied()
                .collect()
        }

        #[test]
        fn char_window_matches_the_element_oracle() {
            for data in datasets() {
                for start in [0, 1, 2, 3, 7, 100] {
                    for end in [None, Some(0), Some(1), Some(3), Some(5), Some(100)] {
                        for capacity in CAPACITIES {
                            let mut out = Vec::new();
                            char_window(
                                BufReader::with_capacity(capacity, data.as_slice()),
                                &mut out,
                                start,
                                end,
                            )
                            .unwrap();
                            assert_eq!(
                                out,
                                expected(&data, start, end, 1),
                                "start={start} end={end:?} capacity={capacity} data={data:02x?}"
                            );
                        }
                    }
                }
            }
        }

        #[test]
        fn char_stepped_matches_the_element_oracle() {
            for data in datasets() {
                for step in [1, 2, 3, 7] {
                    for start in [0, 1, 3, 100] {
                        for end in [None, Some(0), Some(2), Some(5), Some(100)] {
                            for capacity in CAPACITIES {
                                let mut out = Vec::new();
                                char_stepped(
                                    BufReader::with_capacity(capacity, data.as_slice()),
                                    &mut out,
                                    start,
                                    end,
                                    nz(step),
                                )
                                .unwrap();
                                assert_eq!(
                                    out,
                                    expected(&data, start, end, step),
                                    "start={start} end={end:?} step={step} capacity={capacity} data={data:02x?}"
                                );
                            }
                        }
                    }
                }
            }
        }

        #[test]
        fn char_tail_matches_the_element_oracle() {
            for data in datasets() {
                let total = Utf8Elements::new(&data).count();
                for back in [1, 2, 3, 5, 20] {
                    // The plan's invariants: an absolute end is >= 1, a
                    // tail-relative end is < back.
                    let mut ends = vec![
                        None,
                        Some(SliceIndex::FromStart(1)),
                        Some(SliceIndex::FromStart(3)),
                        Some(SliceIndex::FromStart(100)),
                    ];
                    for m in 1..back {
                        ends.push(Some(SliceIndex::FromEnd(nz(m))));
                    }
                    for end in ends {
                        for step in [1, 2, 3] {
                            for capacity in CAPACITIES {
                                let mut out = Vec::new();
                                char_tail(
                                    BufReader::with_capacity(capacity, data.as_slice()),
                                    &mut out,
                                    nz(back),
                                    end,
                                    nz(step),
                                )
                                .unwrap();
                                let start =
                                    SliceIndex::FromEnd(nz(back)).resolve(total as u64) as usize;
                                let stop = end.map_or(total, |e| e.resolve(total as u64) as usize);
                                assert_eq!(
                                    out,
                                    expected(&data, start, Some(stop), step),
                                    "back={back} end={end:?} step={step} capacity={capacity} data={data:02x?}"
                                );
                            }
                        }
                    }
                }
            }
        }

        #[test]
        fn char_lag_matches_the_element_oracle() {
            for data in datasets() {
                let total = Utf8Elements::new(&data).count();
                for start in [0, 1, 3] {
                    for back in [1, 2, 5] {
                        for step in [1, 2, 3] {
                            for capacity in CAPACITIES {
                                let mut out = Vec::new();
                                char_lag(
                                    BufReader::with_capacity(capacity, data.as_slice()),
                                    &mut out,
                                    start,
                                    nz(back),
                                    nz(step),
                                )
                                .unwrap();
                                // Relative element i (after the start skip)
                                // survives iff i + back < N; the stride
                                // applies to i.
                                let keep = total.saturating_sub(start).saturating_sub(back);
                                let reference: Vec<u8> = Utf8Elements::new(&data)
                                    .skip(start)
                                    .take(keep)
                                    .step_by(step)
                                    .flatten()
                                    .copied()
                                    .collect();
                                assert_eq!(
                                    out,
                                    reference,
                                    "start={start} back={back} step={step} capacity={capacity} data={data:02x?}"
                                );
                            }
                        }
                    }
                }
            }
        }

        // The F0 90 | 41 straddle: bytes consumed past a block edge before
        // the sequence was refuted must stay in stream order on every path.
        #[test]
        fn straddled_bytes_survive_on_every_driver() {
            let data: &[u8] = &[0xF0, 0x90, 0x41, 0x42];
            for capacity in [1, 2, 3] {
                let mut out = Vec::new();
                char_window(BufReader::with_capacity(capacity, data), &mut out, 1, None).unwrap();
                assert_eq!(out, [0x90, 0x41, 0x42], "window capacity={capacity}");

                let mut out = Vec::new();
                char_stepped(
                    BufReader::with_capacity(capacity, data),
                    &mut out,
                    0,
                    None,
                    nz(2),
                )
                .unwrap();
                assert_eq!(out, [0xF0, 0x41], "stepped capacity={capacity}");
            }
        }

        #[test]
        fn for_each_matches_in_memory_elements() {
            for data in datasets() {
                let expected: Vec<Vec<u8>> = Utf8Elements::new(&data).map(<[u8]>::to_vec).collect();
                for capacity in CAPACITIES {
                    let mut reader = BufReader::with_capacity(capacity, data.as_slice());
                    let mut seen = Vec::new();
                    Scanner::new()
                        .for_each(&mut reader, |element| {
                            seen.push(element.to_vec());
                            Ok(true)
                        })
                        .unwrap();
                    assert_eq!(seen, expected, "capacity {capacity} data={data:02x?}");
                }
            }
        }

        #[test]
        fn for_each_stops_when_told() {
            let mut reader = BufReader::new("あいうえお".as_bytes());
            let mut seen = 0;
            Scanner::new()
                .for_each(&mut reader, |_| {
                    seen += 1;
                    Ok(seen < 3)
                })
                .unwrap();
            assert_eq!(seen, 3);
        }
    }
}
