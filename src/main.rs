#![doc = include_str!("../README.md")]

mod cli;
mod ext;
mod range;

use crate::{
    ext::{
        read_all_with_record_limit, slice_lag, slice_lag_with_record_limit, slice_stepped,
        slice_tail, slice_tail_with_record_limit, slice_window, Byte, Bytes,
    },
    range::{DeferredPlan, Plan, ReversePlan, SliceIndex, SlicePlan, SliceRange},
};
use clap::{CommandFactory, Parser};
use std::{
    fs,
    io::{self, stdin, stdout, BufRead, Read, Seek, SeekFrom, Write},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::ExitCode,
};

const WRITE_BUF_SIZE: usize = 8 * 1024;

enum SliceMode<'b> {
    Lines,
    Bytes,
    /// Non-empty by construction: `slice_mode` folds the empty delimiter into
    /// `Bytes`, so the delimiter drivers never see an empty shape.
    Custom(&'b [u8]),
}

/// Classify the slicing mode from the resolved flags. An empty `--delimiter`
/// splits one byte per chunk — identical to byte mode under every plan — so it
/// is normalized to `Bytes` here, reaching the seek/copy byte fast paths.
#[inline]
fn slice_mode(bytes: bool, delimiter: Option<&[u8]>) -> SliceMode<'_> {
    if bytes {
        return SliceMode::Bytes;
    }
    match delimiter {
        Some([]) => SliceMode::Bytes,
        Some(delimiter) => SliceMode::Custom(delimiter),
        None => SliceMode::Lines,
    }
}

/// `--translate` only needs the element kind, not the delimiter bytes, so it
/// classifies through `slice_mode` rather than re-deriving the taxonomy — an
/// empty `--delimiter` must reach `Bytes` here too, not `Custom`.
impl From<&SliceMode<'_>> for range::TranslateMode {
    #[inline]
    fn from(mode: &SliceMode<'_>) -> Self {
        match mode {
            SliceMode::Lines => range::TranslateMode::Lines,
            SliceMode::Bytes => range::TranslateMode::Bytes,
            SliceMode::Custom(_) => range::TranslateMode::Custom,
        }
    }
}

#[inline]
fn buf_reader<R: Read>(reader: R, capacity: Option<usize>) -> io::BufReader<R> {
    if let Some(capacity) = capacity {
        io::BufReader::with_capacity(capacity, reader)
    } else {
        io::BufReader::new(reader)
    }
}

#[inline]
fn buf_writer<W: Write>(writer: W, capacity: Option<usize>) -> io::BufWriter<W> {
    if let Some(capacity) = capacity {
        io::BufWriter::with_capacity(capacity, writer)
    } else {
        io::BufWriter::new(writer)
    }
}

#[inline]
fn delimit_window<R: BufRead, W: Write>(
    input: R,
    output: W,
    delimiter: &[u8],
    start: usize,
    end: Option<usize>,
) -> io::Result<()> {
    debug_assert!(!delimiter.is_empty(), "empty delimiter is byte mode");
    match delimiter {
        &[b] => slice_window(Byte(b), input, output, start, end),
        multi => slice_window(Bytes::new(multi), input, output, start, end),
    }
}

#[inline]
fn delimit_stepped<R: BufRead, W: Write>(
    input: R,
    output: W,
    delimiter: &[u8],
    start: usize,
    end: Option<usize>,
    step: NonZeroUsize,
) -> io::Result<()> {
    debug_assert!(!delimiter.is_empty(), "empty delimiter is byte mode");
    match delimiter {
        &[b] => slice_stepped(Byte(b), input, output, start, end, step),
        multi => slice_stepped(Bytes::new(multi), input, output, start, end, step),
    }
}

// Byte-mode stepped path: emits the bytes at indices i in [start, min(end, len))
// with (i - start) % step == 0. `end` is an absolute index from the stream
// start, so start >= end selects nothing.
fn byte_mode<R: BufRead, W: Write>(
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
    discard(&mut input, start as u64)?;
    let step = step.get();
    let mut buf = Vec::with_capacity(WRITE_BUF_SIZE);
    // Offset of the next selected byte within the unread stream; carried
    // across fill_buf blocks so the stride stays aligned to `start`.
    let mut phase = 0;
    loop {
        let block = match input.fill_buf() {
            Ok([]) => break,
            Ok(block) => block,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        };
        let limit = match remaining {
            Some(remaining) => block.len().min(remaining),
            None => block.len(),
        };
        while phase < limit {
            buf.push(block[phase]);
            if buf.len() == WRITE_BUF_SIZE {
                output.write_all(&buf)?;
                buf.clear();
            }
            phase = phase.saturating_add(step);
        }
        phase -= limit;
        input.consume(limit);
        if let Some(remaining) = &mut remaining {
            *remaining -= limit;
            if *remaining == 0 {
                break;
            }
        }
    }
    if !buf.is_empty() {
        output.write_all(&buf)?;
    }
    output.flush()
}

// stdin and pipes are not Seek; advance past `start` bytes by consuming them.
#[inline]
fn discard<R: BufRead>(reader: &mut R, mut n: u64) -> io::Result<()> {
    while n > 0 {
        let consumed = {
            let buf = match reader.fill_buf() {
                Ok([]) => break,
                Ok(buf) => buf,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            buf.len().min(n as usize)
        };
        reader.consume(consumed);
        n -= consumed as u64;
    }
    Ok(())
}

// Byte-mode unit-step fast-path: emits input bytes [start, min(end, len)),
// matching IteratorExt::slice's take(end).skip(start) ordering — `end` is an
// absolute index from the stream start, so start >= end yields an empty window.
#[inline]
fn byte_window<R, W, S>(
    mut input: R,
    mut output: W,
    start: usize,
    end: Option<usize>,
    skip: S,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    S: Fn(&mut R, u64) -> io::Result<()>,
{
    if start > 0 {
        skip(&mut input, start as u64)?;
    }
    match end {
        Some(end) => {
            let len = end.saturating_sub(start) as u64;
            io::copy(&mut (&mut input).take(len), &mut output)?;
        }
        None => {
            io::copy(&mut input, &mut output)?;
        }
    }
    output.flush()
}

#[inline]
fn explain_mode<W: Write>(mut output: W, range: &SliceRange, unit: &str) -> io::Result<()> {
    output.write_all(range.explain(unit).as_bytes())?;
    output.flush()
}

#[inline]
fn translate_mode<W: Write>(
    mut output: W,
    range: &SliceRange,
    mode: range::TranslateMode,
    dialect: range::TranslateDialect,
) -> io::Result<()> {
    output.write_all(range.translate(mode, dialect).as_bytes())?;
    output.flush()
}

// The completion scripts and the man page must name the installed binary
// (`slice`), not the crate (`slice-command`); Args pins the Command name to
// CARGO_BIN_NAME, so both generators inherit it.
fn generate_mode<W: Write>(mut output: W, kind: cli::Generate) -> io::Result<()> {
    use clap_complete::aot::{Generator, Shell};
    let mut cmd = cli::Args::command();
    let shell = match kind {
        cli::Generate::CompleteBash => Shell::Bash,
        cli::Generate::CompleteZsh => Shell::Zsh,
        cli::Generate::CompleteFish => Shell::Fish,
        cli::Generate::CompletePowershell => Shell::PowerShell,
        cli::Generate::Man => {
            clap_mangen::Man::new(cmd).render(&mut output)?;
            return output.flush();
        }
    };
    // try_generate instead of clap_complete::aot::generate: the latter panics
    // on writer failure, while an io::Result keeps a closed pipe a quiet
    // success like every other output path (see stdout_status).
    let name = cmd.get_name().to_owned();
    cmd.set_bin_name(name);
    cmd.build();
    shell.try_generate(&cmd, &mut output)?;
    output.flush()
}

#[inline]
fn copy_mode<R: BufRead, W: Write>(mut input: R, mut output: W) -> io::Result<()> {
    io::copy(&mut input, &mut output)?;
    output.flush()
}

#[inline]
fn apply<R, W, S>(
    mode: &SliceMode,
    input: R,
    mut output: W,
    plan: SlicePlan,
    skip: S,
) -> io::Result<()>
where
    R: BufRead,
    W: Write,
    S: Fn(&mut R, u64) -> io::Result<()>,
{
    match plan {
        SlicePlan::Empty => output.flush(),
        SlicePlan::Copy => copy_mode(input, output),
        SlicePlan::Window { start, end } => match mode {
            SliceMode::Lines => slice_window(Byte(b'\n'), input, output, start, end),
            SliceMode::Bytes => byte_window(input, output, start, end, skip),
            SliceMode::Custom(delimiter) => delimit_window(input, output, delimiter, start, end),
        },
        SlicePlan::Stepped { start, end, step } => match mode {
            SliceMode::Lines => slice_stepped(Byte(b'\n'), input, output, start, end, step),
            SliceMode::Bytes => byte_mode(input, output, start, end, step),
            SliceMode::Custom(delimiter) => {
                delimit_stepped(input, output, delimiter, start, end, step)
            }
        },
    }
}

// Emit the stride-selected bytes of one confirmed segment. `phase` is the
// offset of the next selected byte relative to the segment start; on return it
// is relative to the segment end, so consecutive segments share one stride.
#[inline]
fn emit_run<W: Write>(
    seg: &[u8],
    step: usize,
    phase: &mut usize,
    buf: &mut Vec<u8>,
    output: &mut W,
) -> io::Result<()> {
    if step == 1 {
        return output.write_all(seg);
    }
    let mut p = *phase;
    while p < seg.len() {
        buf.push(seg[p]);
        if buf.len() == WRITE_BUF_SIZE {
            output.write_all(buf)?;
            buf.clear();
        }
        p = p.saturating_add(step);
    }
    *phase = p - seg.len();
    Ok(())
}

/// GNU `head -c -N` equivalent generalized with start/step: bytes ride a ring
/// of the last `back` bytes seen and one is confirmed (emitted, stride
/// permitting) once `back` more bytes arrive, so at EOF the ring holds exactly
/// the dropped tail. Runs when the resolve/seek fast path is unavailable
/// (stdin, FIFOs, sizeless files) and assumes plain `BufRead`, so the leading
/// `start` bytes are read-discarded.
fn byte_lag<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    start: usize,
    back: NonZeroUsize,
    step: NonZeroUsize,
) -> io::Result<()> {
    discard(&mut input, start as u64)?;
    let m = back.get();
    let step = step.get();
    let mut ring: Vec<u8> = Vec::new();
    // Bytes appended to the ring. Its window is [filled - ring.len(), filled)
    // at slots p % m; everything before the window start is already emitted.
    let mut filled: u64 = 0;
    let mut buf = Vec::new();
    let mut phase = 0;
    loop {
        let used = {
            let block = match input.fill_buf() {
                Ok([]) => break,
                Ok(block) => block,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            let n = block.len();
            // The block confirms every byte more than m behind the new total:
            // the ring window's prefix, then — when the block outsizes the
            // ring — the block's own prefix. Both are emitted before
            // ring_extend overwrites exactly those slots.
            let window_start = filled - ring.len() as u64;
            let confirmed = (filled + n as u64).saturating_sub(m as u64);
            let from_ring =
                (confirmed.saturating_sub(window_start)).min(ring.len() as u64) as usize;
            if from_ring > 0 {
                let pos = (window_start % m as u64) as usize;
                let first = from_ring.min(ring.len() - pos);
                emit_run(
                    &ring[pos..pos + first],
                    step,
                    &mut phase,
                    &mut buf,
                    &mut output,
                )?;
                emit_run(
                    &ring[..from_ring - first],
                    step,
                    &mut phase,
                    &mut buf,
                    &mut output,
                )?;
            }
            if confirmed > filled {
                let prefix = (confirmed - filled) as usize;
                emit_run(&block[..prefix], step, &mut phase, &mut buf, &mut output)?;
            }
            ring_extend(&mut ring, m, filled, block);
            filled += n as u64;
            n
        };
        input.consume(used);
    }
    if !buf.is_empty() {
        output.write_all(&buf)?;
    }
    output.flush()
}

/// Append `data` to the circular buffer of the last `k` bytes: the byte at
/// absolute index p lives at slot p % k (`filled` counts the bytes appended
/// before this call). The middle of an oversized block is never copied — only
/// up to `k - ring.len()` leading bytes (growth) and the last k bytes are
/// written, in at most two `copy_from_slice` once the ring is full.
#[inline]
fn ring_extend(ring: &mut Vec<u8>, k: usize, mut filled: u64, mut data: &[u8]) {
    // Grown lazily toward k so a huge `-k:` never preallocates past the bytes
    // actually seen; while growing, slot p % k = p = ring.len().
    if ring.len() < k {
        let grow = data.len().min(k - ring.len());
        ring.extend_from_slice(&data[..grow]);
        filled += grow as u64;
        data = &data[grow..];
        if data.is_empty() {
            return;
        }
    }
    let keep = data.len().min(k);
    let tail = &data[data.len() - keep..];
    let pos = ((filled + (data.len() - keep) as u64) % k as u64) as usize;
    let first = keep.min(k - pos);
    ring[pos..pos + first].copy_from_slice(&tail[..first]);
    ring[..keep - first].copy_from_slice(&tail[first..]);
}

/// `tail -c N` equivalent generalized with end/step: the last `back` bytes
/// seen ride a circular buffer; EOF fixes the length and the resolve
/// arithmetic picks the emitted span, always within the ring's window. An
/// absolute `end` freezes the ring once reached — the remainder is counted
/// without copying.
fn byte_tail<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    back: NonZeroUsize,
    end: Option<SliceIndex>,
    step: NonZeroUsize,
) -> io::Result<()> {
    let k = back.get();
    let bound = match end {
        Some(SliceIndex::FromStart(end)) => Some(end as u64),
        _ => None,
    };
    let mut ring: Vec<u8> = Vec::new();
    // Bytes appended to the ring (capped at `bound`) / consumed from input.
    let mut filled: u64 = 0;
    let mut total: u64 = 0;
    loop {
        let used = {
            let block = match input.fill_buf() {
                Ok([]) => break,
                Ok(block) => block,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            };
            let take = match bound {
                Some(end) => block.len().min((end - filled) as usize),
                None => block.len(),
            };
            ring_extend(&mut ring, k, filled, &block[..take]);
            filled += take as u64;
            block.len()
        };
        input.consume(used);
        total += used as u64;
    }
    let start = SliceIndex::FromEnd(back).resolve(total);
    let end = end.map_or(total, |end| end.resolve(total));
    if step.get() == 1 {
        // The selected span is contiguous in the ring, split at most once at
        // the wrap boundary.
        if start < end {
            let pos = (start % k as u64) as usize;
            let count = (end - start) as usize;
            let first = count.min(k - pos);
            output.write_all(&ring[pos..pos + first])?;
            output.write_all(&ring[..count - first])?;
        }
    } else {
        let step = step.get() as u64;
        let mut buf = Vec::with_capacity(WRITE_BUF_SIZE);
        let mut p = start;
        while p < end {
            buf.push(ring[(p % k as u64) as usize]);
            if buf.len() == WRITE_BUF_SIZE {
                output.write_all(&buf)?;
                buf.clear();
            }
            p = p.saturating_add(step);
        }
        if !buf.is_empty() {
            output.write_all(&buf)?;
        }
    }
    output.flush()
}

#[inline]
fn delimit_tail<R: BufRead, W: Write>(
    input: R,
    output: W,
    delimiter: &[u8],
    back: NonZeroUsize,
    end: Option<SliceIndex>,
    step: NonZeroUsize,
    max_record_size: Option<usize>,
) -> io::Result<()> {
    debug_assert!(!delimiter.is_empty(), "empty delimiter is byte mode");
    match delimiter {
        &[b] if max_record_size.is_some() => {
            slice_tail_with_record_limit(Byte(b), input, output, back, end, step, max_record_size)
        }
        &[b] => slice_tail(Byte(b), input, output, back, end, step),
        multi if max_record_size.is_some() => slice_tail_with_record_limit(
            Bytes::new(multi),
            input,
            output,
            back,
            end,
            step,
            max_record_size,
        ),
        multi => slice_tail(Bytes::new(multi), input, output, back, end, step),
    }
}

#[inline]
fn delimit_lag<R: BufRead, W: Write>(
    input: R,
    output: W,
    delimiter: &[u8],
    start: usize,
    back: NonZeroUsize,
    step: NonZeroUsize,
    max_record_size: Option<usize>,
) -> io::Result<()> {
    debug_assert!(!delimiter.is_empty(), "empty delimiter is byte mode");
    match delimiter {
        &[b] if max_record_size.is_some() => {
            slice_lag_with_record_limit(Byte(b), input, output, start, back, step, max_record_size)
        }
        &[b] => slice_lag(Byte(b), input, output, start, back, step),
        multi if max_record_size.is_some() => slice_lag_with_record_limit(
            Bytes::new(multi),
            input,
            output,
            start,
            back,
            step,
            max_record_size,
        ),
        multi => slice_lag(Bytes::new(multi), input, output, start, back, step),
    }
}

#[inline]
fn apply_deferred<R: BufRead, W: Write>(
    mode: &SliceMode,
    input: R,
    output: W,
    plan: DeferredPlan,
    max_record_size: Option<usize>,
) -> io::Result<()> {
    match plan {
        DeferredPlan::Tail { back, end, step } => match mode {
            SliceMode::Lines if max_record_size.is_some() => slice_tail_with_record_limit(
                Byte(b'\n'),
                input,
                output,
                back,
                end,
                step,
                max_record_size,
            ),
            SliceMode::Lines => slice_tail(Byte(b'\n'), input, output, back, end, step),
            SliceMode::Bytes => byte_tail(input, output, back, end, step),
            SliceMode::Custom(delimiter) => {
                delimit_tail(input, output, delimiter, back, end, step, max_record_size)
            }
        },
        DeferredPlan::Lag { start, back, step } => match mode {
            SliceMode::Lines if max_record_size.is_some() => slice_lag_with_record_limit(
                Byte(b'\n'),
                input,
                output,
                start,
                back,
                step,
                max_record_size,
            ),
            SliceMode::Lines => slice_lag(Byte(b'\n'), input, output, start, back, step),
            SliceMode::Bytes => byte_lag(input, output, start, back, step),
            SliceMode::Custom(delimiter) => {
                delimit_lag(input, output, delimiter, start, back, step, max_record_size)
            }
        },
    }
}

/// The reverse plan buffers the whole input: the first element out is in
/// general the last element in, so unlike Tail's bounded ring no fixed-size
/// window suffices. `--max-record-size` still bounds each record, enforced
/// while reading so an oversized record fails after at most the limit's
/// bytes of it, not after the input was swallowed whole.
fn apply_reverse<R: BufRead, W: Write>(
    mode: &SliceMode,
    mut input: R,
    mut output: W,
    plan: ReversePlan,
    max_record_size: Option<usize>,
) -> io::Result<()> {
    // The record limit is a line/delimiter concept: byte mode ignores it,
    // like the tail-relative byte paths.
    let data = match (mode, max_record_size) {
        (SliceMode::Lines, Some(_)) => {
            read_all_with_record_limit(Byte(b'\n'), input, max_record_size)?
        }
        (SliceMode::Custom(&[b]), Some(_)) => {
            read_all_with_record_limit(Byte(b), input, max_record_size)?
        }
        (SliceMode::Custom(delimiter), Some(_)) => {
            read_all_with_record_limit(Bytes::new(delimiter), input, max_record_size)?
        }
        _ => {
            let mut data = Vec::new();
            input.read_to_end(&mut data)?;
            data
        }
    };
    match mode {
        SliceMode::Bytes => reverse_bytes(&data, &mut output, plan)?,
        SliceMode::Lines => reverse_chunks(&data, &mut output, b"\n", plan)?,
        SliceMode::Custom(delimiter) => reverse_chunks(&data, &mut output, delimiter, plan)?,
    }
    output.flush()
}

fn reverse_bytes<W: Write>(data: &[u8], output: &mut W, plan: ReversePlan) -> io::Result<()> {
    let mut buf = Vec::with_capacity(WRITE_BUF_SIZE);
    for i in plan.indices(data.len()) {
        buf.push(data[i]);
        if buf.len() == WRITE_BUF_SIZE {
            output.write_all(&buf)?;
            buf.clear();
        }
    }
    output.write_all(&buf)
}

/// Emit the selected chunks in descending order under the terminator model:
/// every element is written delimiter-terminated, except that when the
/// input's unterminated final chunk is selected (it is then the first out),
/// its missing delimiter floats to the end of the output.
fn reverse_chunks<W: Write>(
    data: &[u8],
    output: &mut W,
    delimiter: &[u8],
    plan: ReversePlan,
) -> io::Result<()> {
    debug_assert!(!delimiter.is_empty(), "empty delimiter is byte mode");
    let finder = memchr::memmem::Finder::new(delimiter);
    // Content spans, delimiters excluded; only the final chunk can lack one.
    let mut chunks = Vec::new();
    let mut pos = 0;
    while let Some(hit) = finder.find(&data[pos..]) {
        chunks.push((pos, pos + hit));
        pos += hit + delimiter.len();
    }
    let unterminated = pos < data.len();
    if unterminated {
        chunks.push((pos, data.len()));
    }
    let mut selected = plan.indices(chunks.len()).peekable();
    // indices() clamps the walk's origin to the last index, so the walk can
    // include the last chunk only as its first element.
    let all_terminated = !(unterminated && selected.peek() == Some(&(chunks.len() - 1)));
    while let Some(i) = selected.next() {
        let (start, end) = chunks[i];
        output.write_all(&data[start..end])?;
        if selected.peek().is_some() || all_terminated {
            output.write_all(delimiter)?;
        }
    }
    Ok(())
}

fn report_error(path: &Path, err: &io::Error) {
    eprintln!("slice: {}: {}", path.display(), err);
}

// A broken pipe means a downstream consumer (e.g. `head`) closed early; that is
// a normal stop request, not a failure, so we abort quietly without touching
// the exit status.
#[inline]
fn is_broken_pipe(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::BrokenPipe
}

// Exit status for output written without a file context (stdout from stdin or
// --explain): broken pipe is a quiet success, any other error is reported.
fn stdout_status(result: io::Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(err) if is_broken_pipe(&err) => true,
        Err(err) => {
            eprintln!("slice: {err}");
            false
        }
    }
}

#[inline]
fn multi<W: Write, R: BufRead, IW: Fn(fs::File) -> R, F: Fn(R, &mut W) -> io::Result<()>>(
    targets: &[PathBuf],
    mut out: W,
    input_wrapper: IW,
    print_header: bool,
    f: F,
) -> bool {
    let mut ok = true;
    for target in targets {
        // Open before printing the header so an unopenable file gets an error
        // on stderr instead of a header, and the remaining files are still
        // processed (same convention as head(1)/tail(1)).
        let file = match fs::File::open(target) {
            Ok(file) => file,
            Err(err) => {
                report_error(target, &err);
                ok = false;
                continue;
            }
        };
        let result = (|| {
            if print_header {
                writeln!(out, "==> {} <==", target.display())?;
            }
            f(input_wrapper(file), &mut out)
        })();
        if let Err(err) = result {
            if is_broken_pipe(&err) {
                return ok;
            }
            report_error(target, &err);
            ok = false;
        }
    }
    ok
}

/// Byte counts are trustworthy only for regular files: FIFOs and procfs-style
/// files report 0 (or lie), and a 0-length regular file streams to the same
/// empty output anyway. A regular file whose st_size lies (sysfs attributes)
/// is taken at its word — the same trust tail(1) places in st_size.
fn regular_len(file: &fs::File) -> Option<u64> {
    let metadata = file.metadata().ok()?;
    (metadata.is_file() && metadata.len() > 0).then_some(metadata.len())
}

fn entry(args: cli::Args) -> bool {
    if let Some(kind) = args.generate {
        return stdout_status(generate_mode(stdout().lock(), kind));
    }
    let io_buffer_size = args.io_buffer_size();
    let max_record_size = args.max_record_size();
    let delimiter = match args.delimiter() {
        Ok(delimiter) => delimiter,
        Err(e) => cli::Args::command()
            .error(clap::error::ErrorKind::ValueValidation, e)
            .exit(),
    };
    let Some(range) = args.range else {
        // clap only waives the required <RANGE> when the exclusive
        // --generate is present, and that case returned above.
        unreachable!("<RANGE> is required when --generate is absent");
    };
    // One classification feeds --explain, --translate, and the slicing
    // dispatch, so the empty delimiter (folded to Bytes by slice_mode) is
    // treated identically by all three — never a "part"/Custom on one path and
    // a byte on another.
    let mode = slice_mode(args.bytes, delimiter.as_deref());
    if args.explain {
        let unit = match mode {
            SliceMode::Bytes => "byte",
            SliceMode::Custom(_) => "part",
            SliceMode::Lines => "line",
        };
        return stdout_status(explain_mode(stdout().lock(), &range, unit));
    }
    if let Some(dialect) = args.translate {
        let tmode = range::TranslateMode::from(&mode);
        return stdout_status(translate_mode(stdout().lock(), &range, tmode, dialect));
    }
    let plan = range.plan();
    if args.files.is_empty() {
        let input = buf_reader(stdin().lock(), io_buffer_size);
        let output = buf_writer(stdout().lock(), io_buffer_size);
        let result = match plan {
            Plan::Resolved(plan) => apply(&mode, input, output, plan, discard),
            Plan::Deferred(deferred) => {
                apply_deferred(&mode, input, output, deferred, max_record_size)
            }
            Plan::Reverse(reverse) => apply_reverse(&mode, input, output, reverse, max_record_size),
        };
        stdout_status(result)
    } else {
        // A single file never gets a header, so -q only matters for 2+ files.
        let print_header = args.files.len() > 1 && !args.quiet_headers;
        let output = buf_writer(stdout().lock(), io_buffer_size);
        multi(
            &args.files,
            output,
            |input| buf_reader(input, io_buffer_size),
            print_header,
            |input: io::BufReader<fs::File>, output| {
                let seek =
                    |r: &mut io::BufReader<fs::File>, n| r.seek(SeekFrom::Start(n)).map(drop);
                match plan {
                    Plan::Resolved(plan) => apply(&mode, input, output, plan, seek),
                    Plan::Deferred(deferred) => {
                        // Byte offsets resolve against the file size, rejoining
                        // the seek/copy fast paths; line/delimiter counts stay
                        // unknowable up front, so those keep streaming.
                        let len = matches!(mode, SliceMode::Bytes)
                            .then(|| regular_len(input.get_ref()))
                            .flatten();
                        match len.and_then(|len| deferred.resolve(len)) {
                            Some(plan) => apply(&mode, input, output, plan, seek),
                            None => apply_deferred(&mode, input, output, deferred, max_record_size),
                        }
                    }
                    Plan::Reverse(reverse) => {
                        apply_reverse(&mode, input, output, reverse, max_record_size)
                    }
                }
            },
        )
    }
}

fn main() -> ExitCode {
    if entry(cli::Args::parse()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::range::Step;
    use std::str::FromStr;

    // The driver helpers below take absolute offsets; tail-relative bounds
    // have their own deferred drivers and never reach them.
    fn bounds(range: &SliceRange) -> (usize, Option<usize>) {
        let start = match range.start {
            SliceIndex::FromStart(start) => start,
            SliceIndex::FromEnd(_) => panic!("helper drives head-relative ranges"),
        };
        let end = range.end.map(|end| match end {
            SliceIndex::FromStart(end) => end,
            SliceIndex::FromEnd(_) => panic!("helper drives head-relative ranges"),
        });
        (start, end)
    }

    fn resolved_plan_of(range: &SliceRange) -> SlicePlan {
        match range.plan() {
            Plan::Resolved(plan) => plan,
            other => panic!("expected a statically resolved plan, planned {other:?}"),
        }
    }

    fn resolved_plan(range: &str) -> SlicePlan {
        resolved_plan_of(&SliceRange::from_str(range).unwrap())
    }

    fn reverse_plan(range: &str) -> ReversePlan {
        match SliceRange::from_str(range).unwrap().plan() {
            Plan::Reverse(reverse) => reverse,
            other => panic!("{range} must classify as reverse, planned {other:?}"),
        }
    }

    mod translate_classification {
        use super::*;
        use crate::range::TranslateMode;

        // The translate taxonomy must mirror slice_mode: an empty delimiter
        // classifies as Bytes, not Custom.
        #[test]
        fn mirrors_slice_mode_including_empty_delimiter() {
            assert_eq!(
                TranslateMode::from(&slice_mode(true, None)),
                TranslateMode::Bytes
            );
            assert_eq!(
                TranslateMode::from(&slice_mode(false, None)),
                TranslateMode::Lines
            );
            assert_eq!(
                TranslateMode::from(&slice_mode(false, Some(&b","[..]))),
                TranslateMode::Custom
            );
            assert_eq!(
                TranslateMode::from(&slice_mode(false, Some(&b""[..]))),
                TranslateMode::Bytes
            );
        }
    }

    mod line {
        use super::*;

        fn lined(input: &[u8], range: &str) -> Vec<u8> {
            let range = SliceRange::from_str(range).unwrap();
            let (start, end) = bounds(&range);
            let mut out = Vec::new();
            slice_stepped(
                Byte(b'\n'),
                input,
                &mut out,
                start,
                end,
                range.step.magnitude(),
            )
            .expect("");
            out
        }

        #[test]
        fn empty() {
            assert_eq!(lined(b"", "::"), b"");
        }

        mod one_line {
            use super::*;

            const INPUT: &[u8] = b"slice command is simple string slicing command.\n";

            #[test]
            fn no_slice() {
                assert_eq!(lined(INPUT, "::"), INPUT);
            }

            #[test]
            fn skip_first() {
                assert_eq!(lined(INPUT, "1:"), b"");
            }

            #[test]
            fn skip_over_input() {
                assert_eq!(lined(INPUT, "2:"), b"");
            }

            #[test]
            fn drop_tail() {
                assert_eq!(lined(INPUT, ":0"), b"");
            }

            #[test]
            fn step_two_slice() {
                assert_eq!(lined(INPUT, "::2"), INPUT);
            }
        }

        mod multi_line {
            use super::*;

            const INPUT: &[u8] =
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n";

            #[test]
            fn no_slice() {
                assert_eq!(lined(INPUT, "::"), INPUT);
            }

            #[test]
            fn skip_first() {
                assert_eq!(lined(INPUT, "1:"), b"Like a python slice syntax.\n");
            }

            #[test]
            fn drop_last() {
                assert_eq!(
                    lined(INPUT, ":1"),
                    b"slice command is simple string slicing command.\n"
                );
            }

            #[test]
            fn step_two_slice() {
                assert_eq!(
                    lined(&INPUT.repeat(5), "::2"),
                    b"slice command is simple string slicing command.\n".repeat(5)
                );
            }

            #[test]
            fn without_linebreak() {
                let input =
                    b"slice command is simple string slicing command.\nLike a python slice syntax.";
                assert_eq!(lined(input, "::"), input);
            }

            #[test]
            fn binary_stream() {
                let input = b"slice\xaabinary stream\nslice binary\xaastream";
                assert_eq!(lined(input, "::"), input);
            }
        }

        #[test]
        fn bounded_step() {
            assert_eq!(lined(b"l0\nl1\nl2\nl3\nl4\n", "1:4:2"), b"l1\nl3\n");
        }

        #[test]
        fn step_selects_terminal_chunk_without_newline() {
            assert_eq!(lined(b"a\nb\nc", "::2"), b"a\nc");
        }

        #[test]
        fn step_skips_terminal_chunk() {
            assert_eq!(lined(b"a\nb\nc", "1::2"), b"b\n");
        }
    }

    mod explain {
        use super::*;

        // A writer that accepts data but fails when flushed, so a dropped
        // flush result would go unnoticed.
        struct FlushFailWriter;

        impl Write for FlushFailWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::other("flush failed"))
            }
        }

        #[test]
        fn writes_full_explanation() {
            let mut out = Vec::new();
            let range = SliceRange::from_str("1:3").unwrap();
            explain_mode(&mut out, &range, "line").expect("write to a Vec failed");

            assert_eq!(out, range.explain("line").into_bytes());
        }

        #[test]
        fn surfaces_flush_errors() {
            let range = SliceRange::from_str("1:3").unwrap();
            let err = explain_mode(FlushFailWriter, &range, "line")
                .expect_err("a failing flush must surface its error");

            assert_eq!(err.kind(), io::ErrorKind::Other);
        }
    }

    mod generate {
        use super::*;
        use clap::ValueEnum;

        #[test]
        fn every_kind_emits_output() {
            for kind in cli::Generate::value_variants() {
                let mut out = Vec::new();
                generate_mode(&mut out, *kind).expect("generation must succeed");
                assert!(!out.is_empty(), "{kind:?} produced no output");
            }
        }

        // The artifacts must name the installed binary, not the crate
        // (`slice-command`).
        #[test]
        fn bash_completion_names_the_binary() {
            let mut out = Vec::new();
            generate_mode(&mut out, cli::Generate::CompleteBash).expect("");
            let script = String::from_utf8(out).expect("completion scripts are text");
            assert!(script.contains("_slice()"));
            assert!(!script.contains("slice-command"));
        }

        #[test]
        fn man_page_titles_the_binary() {
            let mut out = Vec::new();
            generate_mode(&mut out, cli::Generate::Man).expect("");
            let page = String::from_utf8(out).expect("the man page is roff text");
            assert!(page.contains("\n.TH slice 1"), "missing title: {page}");
            assert!(!page.contains("slice-command"));
        }

        // Both generators must surface writer failures as io::Result (not
        // panic), so a closed pipe stays a quiet success via stdout_status.
        #[test]
        fn surfaces_write_errors() {
            struct FailWriter;
            impl Write for FailWriter {
                fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                    Err(io::Error::other("write failed"))
                }
                fn flush(&mut self) -> io::Result<()> {
                    Err(io::Error::other("flush failed"))
                }
            }
            for kind in [cli::Generate::Man, cli::Generate::CompleteBash] {
                generate_mode(FailWriter, kind)
                    .expect_err("a failing writer must surface its error");
            }
        }
    }

    mod stdout_status {
        use super::*;

        #[test]
        fn broken_pipe_is_success() {
            assert!(stdout_status(Err(io::Error::from(
                io::ErrorKind::BrokenPipe
            ))));
        }

        #[test]
        fn other_errors_fail() {
            assert!(!stdout_status(Err(io::Error::other("boom"))));
        }
    }

    mod broken_pipe {
        use crate::ext::{slice_lag, slice_tail};

        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // A writer that fails every write with BrokenPipe, mimicking a
        // downstream consumer (e.g. `head`) that closed the pipe early.
        struct BrokenPipeWriter;

        impl Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }

        static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

        fn temp_file(contents: &[u8]) -> PathBuf {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "slice-broken-pipe-{}-{}.txt",
                std::process::id(),
                id
            ));
            fs::write(&path, contents).expect("write temp file");
            path
        }

        #[test]
        fn aborts_quietly_and_reports_success() {
            let file = temp_file(b"line one\nline two\n");
            let ok = multi(
                std::slice::from_ref(&file),
                BrokenPipeWriter,
                io::BufReader::new,
                false,
                |input, output| {
                    slice_stepped(Byte(b'\n'), input, output, 0, None, NonZeroUsize::MIN)
                },
            );
            fs::remove_file(&file).ok();

            assert!(ok, "broken pipe must not fail the exit status");
        }

        #[test]
        fn explain_mode_surfaces_the_error() {
            let range = SliceRange::from_str("1:3").unwrap();
            let err = explain_mode(BrokenPipeWriter, &range, "line")
                .expect_err("a failing writer must surface its error");

            assert!(is_broken_pipe(&err));
        }

        #[test]
        fn preserves_earlier_failure() {
            let missing = std::env::temp_dir().join(format!(
                "slice-broken-pipe-missing-{}.txt",
                std::process::id()
            ));
            fs::remove_file(&missing).ok();
            let readable = temp_file(b"line one\nline two\n");

            // The missing file fails to open (a real error reported to stderr),
            // then the broken pipe aborts the rest; the earlier failure must
            // still be reflected in the returned status.
            let ok = multi(
                &[missing, readable.clone()],
                BrokenPipeWriter,
                io::BufReader::new,
                false,
                |input, output| {
                    slice_stepped(Byte(b'\n'), input, output, 0, None, NonZeroUsize::MIN)
                },
            );
            fs::remove_file(&readable).ok();

            assert!(!ok, "a failure before the broken pipe must be preserved");
        }

        #[test]
        fn byte_window_propagates_broken_pipe() {
            let file = temp_file(b"line one\nline two\n");
            let reader = io::BufReader::new(fs::File::open(&file).expect("open temp file"));
            let range = SliceRange::from_str("0:3").unwrap();
            let (start, end) = bounds(&range);
            let err = byte_window(
                reader,
                BrokenPipeWriter,
                start,
                end,
                |r: &mut io::BufReader<fs::File>, n| r.seek(SeekFrom::Start(n)).map(drop),
            )
            .expect_err("a broken pipe must propagate from the window path");
            fs::remove_file(&file).ok();

            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        }

        #[test]
        fn slice_window_propagates_broken_pipe() {
            let file = temp_file(b"line one\nline two\nline three\n");
            // Both window arms must surface the broken pipe: unbounded `1:`
            // exercises the io::copy tail, bounded `0:3` the read_to loop.
            for range in ["1:", "0:3"] {
                let reader = io::BufReader::new(fs::File::open(&file).expect("open temp file"));
                let range = SliceRange::from_str(range).unwrap();
                let (start, end) = bounds(&range);
                let err = slice_window(Byte(b'\n'), reader, BrokenPipeWriter, start, end)
                    .expect_err("a broken pipe must propagate from slice_window");
                assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
            }
            fs::remove_file(&file).ok();
        }

        #[test]
        fn slice_stepped_propagates_broken_pipe() {
            let file = temp_file(b"line one\nline two\nline three\n");
            let reader = io::BufReader::new(fs::File::open(&file).expect("open temp file"));
            let range = SliceRange::from_str("::2").unwrap();
            let (start, end) = bounds(&range);
            let err = slice_stepped(
                Byte(b'\n'),
                reader,
                BrokenPipeWriter,
                start,
                end,
                range.step.magnitude(),
            )
            .expect_err("a broken pipe must propagate from slice_stepped");
            fs::remove_file(&file).ok();

            assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
        }

        // Lag emission happens mid-stream (a chunk is written once its m-th
        // successor lands), so the failure must surface from inside the loop.
        #[test]
        fn lag_emission_propagates_broken_pipe() {
            let err = slice_lag(
                Byte(b'\n'),
                &b"a\nb\nc\n"[..],
                BrokenPipeWriter,
                0,
                NonZeroUsize::MIN,
                NonZeroUsize::MIN,
            )
            .expect_err("a broken pipe must propagate from slice_lag");
            assert!(is_broken_pipe(&err));

            let err = byte_lag(
                &b"abcdefghij"[..],
                BrokenPipeWriter,
                0,
                NonZeroUsize::MIN,
                NonZeroUsize::MIN,
            )
            .expect_err("a broken pipe must propagate from byte_lag");
            assert!(is_broken_pipe(&err));
        }

        // Reverse emission happens at EOF, after the input was read in full;
        // the failure must still surface from the write loop.
        #[test]
        fn reverse_emission_propagates_broken_pipe() {
            for mode in [SliceMode::Lines, SliceMode::Bytes, SliceMode::Custom(b",")] {
                let err = apply_reverse(
                    &mode,
                    &b"a,b\nc,d\n"[..],
                    BrokenPipeWriter,
                    reverse_plan("::-1"),
                    None,
                )
                .expect_err("a broken pipe must propagate from apply_reverse");
                assert!(is_broken_pipe(&err));
            }
        }

        // Tail emission happens at EOF, after the input was read in full; the
        // failure must still surface from the write loop.
        #[test]
        fn tail_emission_propagates_broken_pipe() {
            let err = slice_tail(
                Byte(b'\n'),
                &b"a\nb\nc\n"[..],
                BrokenPipeWriter,
                NonZeroUsize::MIN,
                None,
                NonZeroUsize::MIN,
            )
            .expect_err("a broken pipe must propagate from slice_tail");
            assert!(is_broken_pipe(&err));

            let err = byte_tail(
                &b"abcdefghij"[..],
                BrokenPipeWriter,
                NonZeroUsize::MIN,
                None,
                NonZeroUsize::MIN,
            )
            .expect_err("a broken pipe must propagate from byte_tail");
            assert!(is_broken_pipe(&err));
        }
    }

    mod byte {
        use super::*;

        const INPUT: &[u8] =
            b"slice command is simple string slicing command.\nLike a python slice syntax.\n";

        fn byted(input: &[u8], range: &str) -> Vec<u8> {
            let range = SliceRange::from_str(range).unwrap();
            let (start, end) = bounds(&range);
            let mut out = Vec::new();
            byte_mode(input, &mut out, start, end, range.step.magnitude()).expect("");
            out
        }

        #[test]
        fn empty() {
            assert_eq!(byted(b"", "::"), b"");
        }

        #[test]
        fn no_slice() {
            assert_eq!(byted(INPUT, "::"), INPUT);
        }

        #[test]
        fn skip_first() {
            assert_eq!(
                byted(INPUT, "10:"),
                b"and is simple string slicing command.\nLike a python slice syntax.\n"
            );
        }

        #[test]
        fn drop_last() {
            assert_eq!(byted(INPUT, ":15"), b"slice command i");
        }

        #[test]
        fn skip_first_and_drop_last() {
            assert_eq!(byted(INPUT, "5:15"), b" command i");
        }

        #[test]
        fn skip_two_slice() {
            assert_eq!(
                byted(INPUT, "::2"),
                b"siecmadi ipesrn lcn omn.Lk  yhnsiesna."
            );
        }

        #[test]
        fn stepped_output_crossing_write_buffer_boundary() {
            let input: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
            // "::2" selects exactly 32 KiB (a multiple of the 8 KiB write
            // buffer), "5::3" leaves a partial final batch.
            for (range, start, step) in [("::2", 0, 2), ("5::3", 5, 3)] {
                let expected: Vec<u8> = input.iter().copied().skip(start).step_by(step).collect();
                assert_eq!(byted(&input, range), expected, "range {range}");
            }
        }

        #[test]
        fn stride_phase_carries_across_single_byte_blocks() {
            // Capacity 1 forces every byte into its own fill_buf block, so the
            // selection only works if the stride phase survives each boundary.
            let reader = io::BufReader::with_capacity(1, b"abcdefghij".as_slice());
            let mut out = Vec::new();
            byte_mode(reader, &mut out, 1, None, NonZeroUsize::new(3).unwrap()).expect("");
            assert_eq!(out, b"beh");
        }

        #[test]
        fn stride_parity_with_iterator_oracle() {
            use crate::ext::IteratorExt;

            // Patterned non-UTF-8 input; a prime length avoids lining up with
            // any of the reader capacities below.
            let input: Vec<u8> = (0..1031u32)
                .map(|i| match i % 7 {
                    0 => 0x00,
                    3 => 0xaa,
                    _ => (i % 251) as u8,
                })
                .collect();
            let ranges = [
                (0, None),
                (5, None),
                (0, Some(13)),
                (3, Some(17)),
                (100, None),
                (0, Some(0)),
                (2000, None),
                (3, Some(5000)),
            ];
            for (start, end) in ranges {
                for step in [1, 2, 3, 7, 250] {
                    let step = NonZeroUsize::new(step).unwrap();
                    let expected: Vec<u8> = input
                        .iter()
                        .copied()
                        .slice(start, end, Some(step))
                        .collect();
                    for capacity in [1, 2, 3, 8192] {
                        let reader = io::BufReader::with_capacity(capacity, input.as_slice());
                        let mut out = Vec::new();
                        byte_mode(reader, &mut out, start, end, step).expect("");
                        assert_eq!(
                            out, expected,
                            "start={start} end={end:?} step={step} capacity={capacity}"
                        );
                    }
                }
            }
        }
    }

    mod byte_window {
        use super::*;

        const FIXTURE: &[u8] =
            b"slice command is simple string slicing command.\nLike a python slice syntax.\n";

        fn windowed(input: &[u8], range: &str) -> Vec<u8> {
            let range = SliceRange::from_str(range).unwrap();
            let (start, end) = bounds(&range);
            let mut out = Vec::new();
            byte_window(input, &mut out, start, end, discard).expect("");
            out
        }

        #[test]
        fn skip_first() {
            assert_eq!(
                windowed(FIXTURE, "10:"),
                b"and is simple string slicing command.\nLike a python slice syntax.\n"
            );
        }

        #[test]
        fn drop_last() {
            assert_eq!(windowed(FIXTURE, ":15"), b"slice command i");
        }

        #[test]
        fn skip_first_and_drop_last() {
            assert_eq!(windowed(FIXTURE, "5:15"), b" command i");
        }

        #[test]
        fn unbounded_from_offset_no_trailing_newline() {
            assert_eq!(windowed(b"abcde", "2:"), b"cde");
        }

        #[test]
        fn start_at_or_past_end_is_empty() {
            assert_eq!(windowed(FIXTURE, "3:1"), b"");
        }

        #[test]
        fn start_past_eof_is_empty() {
            assert_eq!(windowed(FIXTURE, "200:"), b"");
        }

        #[test]
        fn bounded_window_past_eof_stops_at_eof() {
            assert_eq!(windowed(FIXTURE, "70:1000"), b"ntax.\n");
        }

        #[test]
        fn end_beyond_len_stops_at_eof() {
            assert_eq!(windowed(b"abc", "0:5"), b"abc");
        }

        #[test]
        fn empty_input() {
            assert_eq!(windowed(b"", "::"), b"");
            assert_eq!(windowed(b"", "0:5"), b"");
        }

        #[test]
        fn binary_is_preserved_byte_exact() {
            let input = b"sl\xaace\xaabinary stream without newline";
            assert_eq!(
                windowed(input, "2:"),
                b"\xaace\xaabinary stream without newline"
            );
        }
    }

    mod seek_vs_discard {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

        fn temp_file(contents: &[u8]) -> PathBuf {
            let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "slice-seek-vs-discard-{}-{}.txt",
                std::process::id(),
                id
            ));
            fs::write(&path, contents).expect("write temp file");
            path
        }

        fn via_discard(input: &[u8], range: &SliceRange) -> Vec<u8> {
            let (start, end) = bounds(range);
            let mut out = Vec::new();
            byte_window(input, &mut out, start, end, discard).expect("");
            out
        }

        fn via_seek(path: &Path, range: &SliceRange) -> Vec<u8> {
            let (start, end) = bounds(range);
            let reader = io::BufReader::new(fs::File::open(path).expect("open temp file"));
            let mut out = Vec::new();
            byte_window(
                reader,
                &mut out,
                start,
                end,
                |r: &mut io::BufReader<fs::File>, n| r.seek(SeekFrom::Start(n)).map(drop),
            )
            .expect("");
            out
        }

        #[test]
        fn seek_matches_discard() {
            let input =
                b"slice command is simple string slicing command.\nLike a python slice syntax.\n";
            let path = temp_file(input);
            for range in ["10:", "5:15", "0:4", "200:"] {
                let range = SliceRange::from_str(range).unwrap();
                assert_eq!(
                    via_seek(&path, &range),
                    via_discard(input, &range),
                    "seek and discard diverged for {range:?}"
                );
            }
            fs::remove_file(&path).ok();
        }
    }

    mod deferred_resolution {
        use super::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        fn deferred(range: &str) -> DeferredPlan {
            match SliceRange::from_str(range).unwrap().plan() {
                Plan::Deferred(deferred) => deferred,
                other => panic!("{range} must defer, planned {other:?}"),
            }
        }

        // The byte fast path: resolve against the known length, then run the
        // resolved plan with a seeking skip, as entry() does for regular files.
        fn via_resolved(input: &[u8], plan: DeferredPlan) -> Vec<u8> {
            let plan = plan.resolve(input.len() as u64).expect("length fits usize");
            let mut out = Vec::new();
            apply(
                &SliceMode::Bytes,
                io::Cursor::new(input),
                &mut out,
                plan,
                |r: &mut io::Cursor<&[u8]>, n| r.seek(SeekFrom::Start(n)).map(drop),
            )
            .expect("");
            out
        }

        // The streaming fallback entry() takes when the length is unknowable.
        fn via_streaming(input: &[u8], plan: DeferredPlan) -> Vec<u8> {
            let mut out = Vec::new();
            apply_deferred(&SliceMode::Bytes, input, &mut out, plan, None).expect("");
            out
        }

        #[test]
        fn engines_agree_on_oracle_rows() {
            let rows: &[(&[u8], &str, &[u8])] = &[
                (b"abcdefghij", "-5:", b"fghij"),
                (b"abcdefghij", ":-3", b"abcdefg"),
                (b"abcdefghij", "-8:-2:3", b"cf"),
                (b"abcdefghij", "2:-2:2", b"ceg"),
                (b"abcdefghij", "-10:", b"abcdefghij"),
                (b"abc", ":-100", b""),
                (b"abcdefghij", "-100:", b"abcdefghij"),
                (b"abcdefghij", "-5:8", b"fgh"),
                (b"abcdefghij", "-2:100", b"ij"),
                (b"abcdefghij", "5:-8", b""),
                (b"", "-3:", b""),
                (b"sl\xaace\x00bin", "-4:", b"\x00bin"),
            ];
            for &(input, range, expected) in rows {
                let plan = deferred(range);
                assert_eq!(
                    via_resolved(input, plan),
                    expected,
                    "resolved engine for {range}"
                );
                assert_eq!(
                    via_streaming(input, plan),
                    expected,
                    "streaming engine for {range}"
                );
            }
        }

        static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

        #[test]
        fn regular_len_requires_a_nonempty_regular_file() {
            for contents in [&b"abc"[..], b""] {
                let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "slice-regular-len-{}-{}.txt",
                    std::process::id(),
                    id
                ));
                fs::write(&path, contents).expect("write temp file");
                let file = fs::File::open(&path).expect("open temp file");
                let len = regular_len(&file);
                fs::remove_file(&path).ok();
                // An empty file must stream: 0 is also what FIFOs report.
                let expected = (!contents.is_empty()).then_some(contents.len() as u64);
                assert_eq!(len, expected, "contents {contents:?}");
            }
        }
    }

    mod byte_lag {
        use super::*;
        use crate::ext::IteratorExt;

        fn lagged_at(
            input: &[u8],
            start: usize,
            m: usize,
            step: usize,
            capacity: usize,
        ) -> Vec<u8> {
            let reader = io::BufReader::with_capacity(capacity, input);
            let mut out = Vec::new();
            byte_lag(
                reader,
                &mut out,
                start,
                NonZeroUsize::new(m).unwrap(),
                NonZeroUsize::new(step).unwrap(),
            )
            .expect("");
            out
        }

        fn lagged(input: &[u8], start: usize, m: usize, step: usize) -> Vec<u8> {
            lagged_at(input, start, m, step, 8 * 1024)
        }

        #[test]
        fn drops_tail_bytes() {
            assert_eq!(lagged(b"abcdefghij", 0, 3, 1), b"abcdefg");
        }

        #[test]
        fn window_with_stride() {
            assert_eq!(lagged(b"abcdefghij", 2, 2, 2), b"ceg");
        }

        #[test]
        fn back_at_or_past_len_is_empty() {
            assert_eq!(lagged(b"abc", 0, 100, 1), b"");
            assert_eq!(lagged(b"abc", 0, 3, 1), b"");
        }

        #[test]
        fn start_inside_dropped_tail_is_empty() {
            assert_eq!(lagged(b"abcdefghij", 8, 5, 1), b"");
            assert_eq!(lagged(b"abcdefghij", 100, 1, 1), b"");
        }

        #[test]
        fn empty_input() {
            assert_eq!(lagged(b"", 0, 1, 1), b"");
        }

        #[test]
        fn binary_is_preserved_byte_exact() {
            let input = b"sl\xaace\xaabinary\x00stream";
            assert_eq!(lagged(input, 0, 6, 1), b"sl\xaace\xaabinary\x00");
        }

        #[test]
        fn stride_crosses_write_buffer_boundary() {
            // 64 KiB input with m=5 leaves >8 KiB of selected output at step 2,
            // forcing the batch buffer to flush mid-run.
            let input: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
            for step in [1usize, 2, 3] {
                let end = input.len() - 5;
                let expected: Vec<u8> = input
                    .iter()
                    .copied()
                    .slice(0, Some(end), NonZeroUsize::new(step))
                    .collect();
                assert_eq!(lagged(&input, 0, 5, step), expected, "step {step}");
            }
        }

        #[test]
        fn parity_with_iterator_oracle() {
            // Patterned non-UTF-8 input; the prime length avoids lining up
            // with any of the reader capacities below.
            let input: Vec<u8> = (0..1031u32)
                .map(|i| match i % 7 {
                    0 => 0x00,
                    3 => 0xaa,
                    _ => (i % 251) as u8,
                })
                .collect();
            for start in [0usize, 1, 5, 100, 2000] {
                for m in [1usize, 2, 3, 7, 250, 1031, 100_000] {
                    for step in [1usize, 2, 3, 7] {
                        let end = input.len().saturating_sub(m);
                        let expected: Vec<u8> = input
                            .iter()
                            .copied()
                            .slice(start, Some(end), NonZeroUsize::new(step))
                            .collect();
                        // Capacity 1 keeps every block below m (ring growth and
                        // wrap), 8192 covers single blocks larger than m.
                        for capacity in [1, 3, 8192] {
                            assert_eq!(
                                lagged_at(&input, start, m, step, capacity),
                                expected,
                                "start={start} m={m} step={step} capacity={capacity}"
                            );
                        }
                    }
                }
            }
        }

        // Serves its data, then fails instead of reporting EOF: anything the
        // consumer wrote before the error must have been emitted mid-stream.
        struct ErrAtEof<'a>(&'a [u8]);

        impl Read for ErrAtEof<'_> {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                unreachable!("driven through BufRead")
            }
        }

        impl BufRead for ErrAtEof<'_> {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                if self.0.is_empty() {
                    Err(io::Error::other("input failed mid-stream"))
                } else {
                    Ok(self.0)
                }
            }
            fn consume(&mut self, amt: usize) {
                self.0 = &self.0[amt..];
            }
        }

        // Pins the streaming contract: a byte is written as soon as its m-th
        // successor arrives, not at EOF. A buffer-until-EOF rewrite would
        // leave `out` empty when the error surfaces.
        #[test]
        fn streams_confirmed_bytes_before_eof() {
            let mut out = Vec::new();
            let err = byte_lag(
                ErrAtEof(b"abcde"),
                &mut out,
                0,
                NonZeroUsize::new(2).unwrap(),
                NonZeroUsize::MIN,
            )
            .expect_err("the mid-stream failure must surface");
            assert_eq!(err.kind(), io::ErrorKind::Other);
            assert_eq!(out, b"abc");
        }
    }

    mod byte_tail {
        use super::*;
        use crate::ext::IteratorExt;

        fn tailed_at(
            input: &[u8],
            k: usize,
            end: Option<SliceIndex>,
            step: usize,
            capacity: usize,
        ) -> Vec<u8> {
            let reader = io::BufReader::with_capacity(capacity, input);
            let mut out = Vec::new();
            byte_tail(
                reader,
                &mut out,
                NonZeroUsize::new(k).unwrap(),
                end,
                NonZeroUsize::new(step).unwrap(),
            )
            .expect("");
            out
        }

        fn tailed(input: &[u8], k: usize, end: Option<SliceIndex>, step: usize) -> Vec<u8> {
            tailed_at(input, k, end, step, 8 * 1024)
        }

        fn at(end: usize) -> Option<SliceIndex> {
            Some(SliceIndex::FromStart(end))
        }

        fn from_end(m: usize) -> Option<SliceIndex> {
            Some(SliceIndex::FromEnd(NonZeroUsize::new(m).unwrap()))
        }

        #[test]
        fn keeps_tail_bytes() {
            assert_eq!(tailed(b"abcdefghij", 5, None, 1), b"fghij");
        }

        #[test]
        fn tail_relative_end_with_stride() {
            // -8:-2:3 == s[2:8:3]
            assert_eq!(tailed(b"abcdefghij", 8, from_end(2), 3), b"cf");
        }

        #[test]
        fn back_at_or_past_len_keeps_whole_input() {
            assert_eq!(tailed(b"abcdefghij", 10, None, 1), b"abcdefghij");
            assert_eq!(tailed(b"abcdefghij", 100, None, 1), b"abcdefghij");
        }

        #[test]
        fn bounded_end_freezes_ring() {
            // -5:8 == s[5:8]
            assert_eq!(tailed(b"abcdefghij", 5, at(8), 1), b"fgh");
            // -2:1 selects [max(0, L-2), 1): byte 0 for L <= 2, nothing after.
            assert_eq!(tailed(b"a", 2, at(1), 1), b"a");
            assert_eq!(tailed(b"abc", 2, at(1), 1), b"");
            // end >= L never freezes.
            assert_eq!(tailed(b"abcdefghij", 2, at(100), 1), b"ij");
        }

        #[test]
        fn empty_input() {
            assert_eq!(tailed(b"", 3, None, 1), b"");
            assert_eq!(tailed(b"", 3, at(1), 1), b"");
        }

        #[test]
        fn binary_is_preserved_byte_exact() {
            let input = b"sl\xaace\xaabinary\x00stream";
            assert_eq!(tailed(input, 8, None, 1), b"y\x00stream");
        }

        #[test]
        fn stride_crosses_write_buffer_boundary() {
            // k = 40000 at step 2 selects 20000 bytes, forcing the 8 KiB batch
            // buffer to flush mid-run.
            let input: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
            let expected: Vec<u8> = input
                .iter()
                .copied()
                .slice(input.len() - 40000, None, NonZeroUsize::new(2))
                .collect();
            assert_eq!(tailed(&input, 40000, None, 2), expected);
        }

        #[test]
        fn parity_with_iterator_oracle() {
            // Patterned non-UTF-8 input; the prime length avoids lining up
            // with any of the reader capacities below.
            let input: Vec<u8> = (0..1031u32)
                .map(|i| match i % 7 {
                    0 => 0x00,
                    3 => 0xaa,
                    _ => (i % 251) as u8,
                })
                .collect();
            for k in [1usize, 2, 3, 7, 250, 1031, 100_000] {
                for end in [
                    None,
                    at(1),
                    at(500),
                    at(2000),
                    from_end(1),
                    from_end(100),
                    from_end(2000),
                ] {
                    for step in [1usize, 2, 3, 7] {
                        let resolved_end = match end {
                            None => input.len(),
                            Some(SliceIndex::FromStart(end)) => end.min(input.len()),
                            Some(SliceIndex::FromEnd(m)) => input.len().saturating_sub(m.get()),
                        };
                        let expected: Vec<u8> = input
                            .iter()
                            .copied()
                            .slice(
                                input.len().saturating_sub(k),
                                Some(resolved_end),
                                NonZeroUsize::new(step),
                            )
                            .collect();
                        // Capacity 1 keeps every block below k (ring growth
                        // and wrap), 8192 covers single blocks larger than k.
                        for capacity in [1, 3, 8192] {
                            assert_eq!(
                                tailed_at(&input, k, end, step, capacity),
                                expected,
                                "k={k} end={end:?} step={step} capacity={capacity}"
                            );
                        }
                    }
                }
            }
        }
    }

    mod copy {
        use super::*;

        #[test]
        fn empty_input() {
            let mut out = Vec::new();
            copy_mode(b"".as_slice(), &mut out).expect("");
            assert_eq!(out, b"");
        }

        #[test]
        fn verbatim_including_binary_and_missing_eol() {
            let input = b"slice\xaabinary stream\nno trailing eol";
            let mut out = Vec::new();
            copy_mode(input.as_slice(), &mut out).expect("");
            assert_eq!(out, input);
        }
    }

    mod identity_fast_path {
        use super::*;

        const INPUT: &[u8] = b"a,b,c\nd,e,f\n";

        fn applied(mode: SliceMode, range: &str) -> Vec<u8> {
            let mut out = Vec::new();
            apply(&mode, INPUT, &mut out, resolved_plan(range), discard).expect("");
            out
        }

        #[test]
        fn lines_colon() {
            assert_eq!(applied(SliceMode::Lines, ":"), INPUT);
        }

        #[test]
        fn lines_colon_colon() {
            assert_eq!(applied(SliceMode::Lines, "::"), INPUT);
        }

        #[test]
        fn lines_explicit_unit_step() {
            assert_eq!(applied(SliceMode::Lines, "0::1"), INPUT);
        }

        #[test]
        fn bytes() {
            assert_eq!(applied(SliceMode::Bytes, "::"), INPUT);
        }

        #[test]
        fn custom_delimiter() {
            assert_eq!(applied(SliceMode::Custom(b",".as_slice()), "::"), INPUT);
        }

        #[test]
        fn minus_zero_start_is_copy() {
            assert_eq!(resolved_plan("-0:"), SlicePlan::Copy);
            assert_eq!(applied(SliceMode::Lines, "-0:"), INPUT);
        }

        #[test]
        fn non_identity_still_slices() {
            let mut out = Vec::new();
            apply(
                &SliceMode::Lines,
                b"a\nb\nc\n".as_slice(),
                &mut out,
                resolved_plan("1:"),
                discard,
            )
            .expect("");
            assert_eq!(out, b"b\nc\n");
        }

        #[test]
        fn custom_unit_step_matches_stepped_driver() {
            // The unit-step Custom window (delimit_window) must agree with the
            // stepped driver run at step 1 for every delimiter shape.
            fn agree(delimiter: &[u8], input: &[u8], range: &str) -> Vec<u8> {
                let range = SliceRange::from_str(range).unwrap();
                let (start, end) = bounds(&range);
                let mut via_apply = Vec::new();
                apply(
                    &SliceMode::Custom(delimiter),
                    input,
                    &mut via_apply,
                    resolved_plan_of(&range),
                    discard,
                )
                .expect("");
                let mut via_stepped = Vec::new();
                delimit_stepped(
                    input,
                    &mut via_stepped,
                    delimiter,
                    start,
                    end,
                    range.step.magnitude(),
                )
                .expect("");
                assert_eq!(
                    via_apply, via_stepped,
                    "apply vs delimit_stepped for {delimiter:?} {range:?}"
                );
                via_apply
            }
            assert_eq!(agree(b"||", b"a||b||c\n", "1:"), b"b||c\n"); // multi-byte
            assert_eq!(agree(b",", b"a,b,c,", "1:"), b"b,c,"); // single-byte
        }
    }

    mod reverse {
        use super::*;

        // Crosses the WRITE_BUF_SIZE batching boundary: a flush/clear bug in
        // reverse_bytes shows up only past the buffer size.
        #[test]
        fn bytes_batching_survives_buffer_boundary() {
            let data: Vec<u8> = (0..3 * WRITE_BUF_SIZE + 17)
                .map(|i| (i % 251) as u8)
                .collect();
            let mut out = Vec::new();
            reverse_bytes(&data, &mut out, reverse_plan("::-1")).expect("");
            assert_eq!(out, data.iter().rev().copied().collect::<Vec<u8>>());

            let mut out = Vec::new();
            reverse_bytes(&data, &mut out, reverse_plan("::-3")).expect("");
            let expected: Vec<u8> = data.iter().rev().copied().step_by(3).collect();
            assert_eq!(out, expected);
        }

        // The record limit aborts an oversized record during the read; byte
        // mode ignores it like the tail-relative byte paths.
        #[test]
        fn record_limit_applies_to_lines_not_bytes() {
            let input = b"toolong\nab\n";
            let mut out = Vec::new();
            let err = apply_reverse(
                &SliceMode::Lines,
                &input[..],
                &mut out,
                reverse_plan("::-1"),
                Some(4),
            )
            .expect_err("an oversized record must fail the reverse read");
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);

            let mut out = Vec::new();
            apply_reverse(
                &SliceMode::Bytes,
                &input[..],
                &mut out,
                reverse_plan("::-1"),
                Some(4),
            )
            .expect("byte mode ignores the record limit");
            assert_eq!(out, input.iter().rev().copied().collect::<Vec<u8>>());
        }
    }

    mod empty_delimiter {
        use super::*;

        // slice_mode is the production routing entry() uses; the empty
        // delimiter must land in byte mode and produce byte-identical output
        // across every plan shape (Copy, Window, Stepped, Empty, Tail, Lag).
        #[test]
        fn routes_through_byte_machinery() {
            const INPUT: &[u8] = b"slice\xaabinary\nstream";
            for range in [
                "::", "1:", ":3", "2:5", "::2", "1::3", "5:3", ":-2", "-3:", "-4:-1:2", "::-1",
                "5:1:-2",
            ] {
                let plan = SliceRange::from_str(range).unwrap().plan();
                let apply_with = |mode: &SliceMode| {
                    let mut out = Vec::new();
                    match plan {
                        Plan::Resolved(plan) => {
                            apply(mode, INPUT, &mut out, plan, discard).expect("")
                        }
                        Plan::Deferred(deferred) => {
                            apply_deferred(mode, INPUT, &mut out, deferred, None).expect("")
                        }
                        Plan::Reverse(reverse) => {
                            apply_reverse(mode, INPUT, &mut out, reverse, None).expect("")
                        }
                    }
                    out
                };
                assert_eq!(
                    apply_with(&slice_mode(false, Some(b""))),
                    apply_with(&slice_mode(true, None)),
                    "empty delimiter diverged from byte mode for {range}"
                );
            }
        }

        #[test]
        fn classification() {
            assert!(matches!(slice_mode(false, None), SliceMode::Lines));
            assert!(matches!(slice_mode(true, None), SliceMode::Bytes));
            assert!(matches!(slice_mode(false, Some(b"")), SliceMode::Bytes));
            assert!(matches!(
                slice_mode(false, Some(b",")),
                SliceMode::Custom(b",")
            ));
        }
    }

    mod empty_plan {
        use super::*;

        // Any read attempt fails, so a passing test proves the Empty plan never
        // touched the input.
        struct NoReadReader;

        impl Read for NoReadReader {
            fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("input must not be read"))
            }
        }

        impl BufRead for NoReadReader {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                Err(io::Error::other("input must not be read"))
            }
            fn consume(&mut self, _amt: usize) {
                panic!("input must not be consumed");
            }
        }

        fn applied(mode: SliceMode, range: &str) -> Vec<u8> {
            let mut out = Vec::new();
            apply(&mode, NoReadReader, &mut out, resolved_plan(range), discard)
                .expect("an empty plan must succeed without reading input");
            out
        }

        #[test]
        fn lines_emit_nothing_without_reading() {
            assert_eq!(applied(SliceMode::Lines, "5:3"), b"");
            assert_eq!(applied(SliceMode::Lines, ":0"), b"");
        }

        #[test]
        fn bytes_emit_nothing_without_reading() {
            assert_eq!(applied(SliceMode::Bytes, "5:3"), b"");
            assert_eq!(applied(SliceMode::Bytes, "5:5"), b"");
        }

        #[test]
        fn custom_delimiter_emits_nothing_without_reading() {
            assert_eq!(applied(SliceMode::Custom(b",".as_slice()), "5:5"), b"");
        }

        #[test]
        fn stepped_empty_range_emits_nothing_without_reading() {
            assert_eq!(applied(SliceMode::Lines, "5:3:2"), b"");
            assert_eq!(applied(SliceMode::Bytes, "5:3:2"), b"");
        }

        #[test]
        fn static_negative_pairs_emit_nothing_without_reading() {
            assert_eq!(applied(SliceMode::Lines, "-2:-5"), b"");
            assert_eq!(applied(SliceMode::Lines, "-3:-3"), b"");
            assert_eq!(applied(SliceMode::Bytes, "-5:0"), b"");
        }

        // Control: a genuinely tail-relative start has no static answer and
        // must read the input.
        #[test]
        fn tail_relative_start_reads_input() {
            let plan = match SliceRange::from_str("-1:").unwrap().plan() {
                Plan::Deferred(deferred) => deferred,
                other => panic!("-1: must defer, planned {other:?}"),
            };
            let mut out = Vec::new();
            apply_deferred(&SliceMode::Lines, NoReadReader, &mut out, plan, None)
                .expect_err("a tail-relative start must read the input");
        }
    }

    mod slice_window {
        use super::*;
        use crate::ext::Byte;

        fn windowed(input: &[u8], range: &str) -> Vec<u8> {
            let range = SliceRange::from_str(range).unwrap();
            let (start, end) = bounds(&range);
            let mut out = Vec::new();
            slice_window(Byte(b'\n'), input, &mut out, start, end).expect("");
            out
        }

        #[test]
        fn unbounded_from_offset() {
            assert_eq!(windowed(b"a\nb\nc\n", "1:"), b"b\nc\n");
        }

        #[test]
        fn bounded_drop_tail() {
            assert_eq!(windowed(b"a\nb\nc\n", ":1"), b"a\n");
        }

        #[test]
        fn unbounded_last_line_without_newline() {
            assert_eq!(windowed(b"a\nb", "1:"), b"b");
        }

        #[test]
        fn empty_bounded_window() {
            assert_eq!(windowed(b"a\nb\nc\n", ":0"), b"");
        }

        #[test]
        fn skip_past_eof_is_empty() {
            assert_eq!(windowed(b"a\nb\n", "2:"), b"");
        }

        #[test]
        fn empty_input() {
            assert_eq!(windowed(b"", "::"), b"");
            assert_eq!(windowed(b"", "1:"), b"");
            assert_eq!(windowed(b"", "0:3"), b"");
        }
    }
}
