use std::{
    num::{IntErrorKind, NonZeroUsize, ParseIntError},
    str::FromStr,
};

/// One bound of a slice. `FromEnd` is a distance back from the end of input;
/// the lexeme `-0` normalizes to `FromStart(0)` at parse time (Python has no
/// -0), so zero distance is unrepresentable here.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) enum SliceIndex {
    FromStart(usize),
    FromEnd(NonZeroUsize),
}

impl SliceIndex {
    /// Resolve to an absolute offset clamped to [0, len] (Python slice.indices).
    #[inline]
    pub(crate) fn resolve(self, len: u64) -> u64 {
        match self {
            SliceIndex::FromStart(i) => (i as u64).min(len),
            SliceIndex::FromEnd(k) => len.saturating_sub(k.get() as u64),
        }
    }
}

/// A step with its direction. Python's negative step selects in reverse;
/// zero stays unrepresentable (the parse rejects it).
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) enum Step {
    Forward(NonZeroUsize),
    Backward(NonZeroUsize),
}

impl Step {
    #[inline]
    pub(crate) fn magnitude(self) -> NonZeroUsize {
        match self {
            Step::Forward(step) | Step::Backward(step) => step,
        }
    }

    /// Forward constructor for tests; panics on zero.
    #[cfg(test)]
    pub(crate) fn forward(step: usize) -> Step {
        Step::Forward(NonZeroUsize::new(step).expect("test steps are nonzero"))
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub(crate) struct SliceRange {
    /// The parse defaults an absent start to `FromStart(0)` for a forward
    /// step and `FromEnd(1)` (the last element) for a reverse one.
    pub(crate) start: SliceIndex,
    /// `None` means unbounded (run to the end of input; for a reverse step,
    /// back through the first element).
    pub(crate) end: Option<SliceIndex>,
    /// The parse defaults an absent step to `Forward(1)`.
    pub(crate) step: Step,
}

/// What an element is, for `--translate`. Mirrors the `SliceMode` taxonomy in
/// `main.rs` but drops the borrowed delimiter bytes — the translation only
/// needs the kind to pick a tool family.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum TranslateMode {
    Lines,
    Bytes,
    Chars,
    Graphemes,
    Custom,
}

/// Which spelling `--translate` prints. The bare flag resolves to the build
/// target's native toolset (see `cli.rs`), so the value is always the user's
/// explicit choice or that platform default.
// Plain comments, not doc comments: a `///` on a ValueEnum variant makes clap
// expand the `--help` possible-values list (and switch the whole option block to
// the long layout), diverging from `--generate`'s inline style. The dialect
// meanings live in the README and the help text instead.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, clap::ValueEnum)]
pub(crate) enum TranslateDialect {
    // Strictly POSIX.1-portable.
    Posix,
    // POSIX plus the common BSD/macOS extensions (`head -c`).
    Bsd,
    // GNU coreutils (`sed -n 'F~Sp'`, `head -n -N`, `head -c`).
    Gnu,
    // `awk` one-liners.
    Awk,
    // Every dialect, one labelled block each.
    All,
}

/// The four concrete dialects `All` expands into. Separate from the public
/// `TranslateDialect` so the renderer never has to handle `All` recursively.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
enum Dialect {
    Posix,
    Bsd,
    Gnu,
    Awk,
}

/// Which numeric field of a `start:end:step` range failed to parse.
#[derive(Clone, Eq, PartialEq, Debug)]
pub(crate) enum RangeField {
    Start,
    End,
    Step,
}

impl std::fmt::Display for RangeField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RangeField::Start => "start",
            RangeField::End => "end",
            RangeField::Step => "step",
        })
    }
}

/// Error produced while parsing a `SliceRange` from its `start:end:step` form.
#[derive(Clone, Eq, PartialEq, Debug, thiserror::Error)]
pub(crate) enum ParseSliceRangeError {
    #[error("range requires a ':' separator (e.g. '3:4', '3:', or ':3')")]
    MissingColon,
    #[error("invalid {field} value '{value}': {source}")]
    InvalidField {
        field: RangeField,
        value: String,
        source: ParseIntError,
    },
    #[error("a relative end ('+' or '+-') requires a count (e.g. '5:+3' or '5:+-3')")]
    MissingRelativeAmount,
    #[error("a negative step cannot be combined with a relative end ('+' or '+-')")]
    NegativeStepWithRelativeEnd,
    #[error("too many ':' separators in range (expected at most start:end:step)")]
    TooManyParts,
}

/// How a parsed range executes. `SliceRange` stays the `start:end:step` as
/// written; classifying it once lets dispatch match on the processing shape
/// instead of re-testing field combinations at every branch.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum SlicePlan {
    /// Bounded with `start >= end` (`5:3`, `:0`): selects nothing in any mode,
    /// so the input need not be read at all.
    Empty,
    /// Selects the whole input unchanged (`:`, `::`, `0::1`): the output equals
    /// the input byte-for-byte in every mode, so splitting can be skipped in
    /// favor of a verbatim copy.
    Copy,
    /// Unit step with an offset or bound: the selected elements are contiguous,
    /// so the output is the single span `[start, end)`.
    Window { start: usize, end: Option<usize> },
    /// `step > 1` selects non-contiguous elements; only the element-by-element
    /// pipeline can express it.
    Stepped {
        start: usize,
        end: Option<usize>,
        step: NonZeroUsize,
    },
}

/// Whether a parsed range can be classified before any input is seen.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum Plan {
    /// Head-relative or statically decidable: classified once, up front.
    Resolved(SlicePlan),
    /// Tail-relative: resolution needs the input length (per input), or a
    /// streaming buffer when the length is unknowable.
    Deferred(DeferredPlan),
    /// Negative step: the first element out is in general the last element
    /// in, so unlike Tail's bounded ring no fixed-size window suffices —
    /// execution buffers the whole input.
    Reverse(ReversePlan),
}

/// Tail (no output before EOF) and Lag (streams with a fixed delay) are
/// distinct execution mechanisms, so they are separate variants rather than
/// one shape matched at runtime.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) enum DeferredPlan {
    /// `-k:…` — tail-relative start. `plan()` establishes the invariants:
    /// `end == Some(FromEnd(m))` only with `m < back`, `end ==
    /// Some(FromStart(e))` only with `e >= 1` (other pairs are statically
    /// empty).
    Tail {
        back: NonZeroUsize,
        end: Option<SliceIndex>,
        step: NonZeroUsize,
    },
    /// `start:-m…` — head-relative start, tail-relative end.
    Lag {
        start: usize,
        back: NonZeroUsize,
        step: NonZeroUsize,
    },
}

/// A negative-step range. Bounds keep their parsed form; [`Self::indices`]
/// maps them onto a descending index walk once the element count is known.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub(crate) struct ReversePlan {
    /// Descending origin (`FromEnd(1)` is the last element).
    start: SliceIndex,
    /// Exclusive lower bound; `None` runs through the first element.
    end: Option<SliceIndex>,
    /// Magnitude of the negative step.
    step: NonZeroUsize,
}

impl ReversePlan {
    /// The selected indices in output (descending) order; empty when nothing
    /// is selected. These are Python's rules for a negative step, which
    /// differ from [`SliceIndex::resolve`]: an explicit head-relative start
    /// clamps to the last element (`len-1`, not `len`), an end at or past the
    /// last element excludes everything, and a tail-relative bound reaching
    /// past the beginning empties the start but unbounds the end.
    #[inline]
    pub(crate) fn indices(&self, len: usize) -> impl Iterator<Item = usize> {
        // `1..=0` is the empty walk.
        let (first, lower) = self.resolve(len).unwrap_or((0, 1));
        (lower..=first).rev().step_by(self.step.get())
    }

    /// The walk endpoints: descend from `first`, never below `lower`.
    fn resolve(&self, len: usize) -> Option<(usize, usize)> {
        let last = len.checked_sub(1)?;
        let first = match self.start {
            SliceIndex::FromStart(start) => start.min(last),
            SliceIndex::FromEnd(back) => len.checked_sub(back.get())?,
        };
        let lower = match self.end {
            None => 0,
            Some(SliceIndex::FromStart(end)) => end.min(last) + 1,
            Some(SliceIndex::FromEnd(back)) => len.checked_sub(back.get()).map_or(0, |end| end + 1),
        };
        (first >= lower).then_some((first, lower))
    }
}

/// Classify absolute (head-relative) bounds into an execution plan.
#[inline]
fn classify(start: usize, end: Option<usize>, step: NonZeroUsize) -> SlicePlan {
    // Checked before step: no step can select anything from `start >= end`.
    if end.is_some_and(|end| start >= end) {
        SlicePlan::Empty
    } else if step.get() > 1 {
        SlicePlan::Stepped { start, end, step }
    } else if start == 0 && end.is_none() {
        SlicePlan::Copy
    } else {
        SlicePlan::Window { start, end }
    }
}

impl DeferredPlan {
    /// Absolutize against a known length and classify through the same rules
    /// as head-relative ranges. An end at or past `len` normalizes to
    /// unbounded, which re-enables the Copy / unbounded io::copy fast paths.
    /// `None` only when the offsets do not fit usize (32-bit, >4GiB): callers
    /// stream instead.
    pub(crate) fn resolve(&self, len: u64) -> Option<SlicePlan> {
        let (start, end, step) = match *self {
            DeferredPlan::Tail { back, end, step } => (SliceIndex::FromEnd(back), end, step),
            DeferredPlan::Lag { start, back, step } => (
                SliceIndex::FromStart(start),
                Some(SliceIndex::FromEnd(back)),
                step,
            ),
        };
        let start = start.resolve(len);
        let end = end.map(|end| end.resolve(len)).filter(|&end| end < len);
        Some(classify(
            usize::try_from(start).ok()?,
            end.map(usize::try_from).transpose().ok()?,
            step,
        ))
    }
}

impl SliceRange {
    #[inline]
    pub(crate) fn plan(&self) -> Plan {
        use SliceIndex::*;
        let step = match self.step {
            Step::Backward(step) => {
                return match (self.start, self.end) {
                    // Descending from at or below the end selects nothing for
                    // every length.
                    (FromStart(s), Some(FromStart(e))) if s <= e => {
                        Plan::Resolved(SlicePlan::Empty)
                    }
                    // L-k down to L-m descending: empty for every length when k >= m.
                    (FromEnd(k), Some(FromEnd(m))) if k >= m => Plan::Resolved(SlicePlan::Empty),
                    // An end 1 from the end stops at L-1, excluding every
                    // index a descending walk can reach.
                    (_, Some(FromEnd(m))) if m.get() == 1 => Plan::Resolved(SlicePlan::Empty),
                    (start, end) => Plan::Reverse(ReversePlan { start, end, step }),
                };
            }
            Step::Forward(step) => step,
        };
        match (self.start, self.end) {
            (FromStart(start), None) => Plan::Resolved(classify(start, None, step)),
            (FromStart(start), Some(FromStart(end))) => {
                Plan::Resolved(classify(start, Some(end), step))
            }
            // [L-k, L-m) is empty for every length when k <= m.
            (FromEnd(k), Some(FromEnd(m))) if k <= m => Plan::Resolved(SlicePlan::Empty),
            // `-k:0` selects nothing for every length, like `:0`.
            (FromEnd(_), Some(FromStart(0))) => Plan::Resolved(SlicePlan::Empty),
            (FromEnd(back), end) => Plan::Deferred(DeferredPlan::Tail { back, end, step }),
            (FromStart(start), Some(FromEnd(back))) => {
                Plan::Deferred(DeferredPlan::Lag { start, back, step })
            }
        }
    }

    /// Render a human-readable description of what this resolved range selects,
    /// without reading any input. `unit` names the elements (e.g. "line").
    pub(crate) fn explain(&self, unit: &str) -> String {
        let step = match self.step {
            Step::Backward(step) => return explain_reverse(self.start, self.end, step.get(), unit),
            Step::Forward(step) => step.get(),
        };
        match (self.start, self.end) {
            (SliceIndex::FromStart(start), None) => explain_resolved(start, None, step, unit),
            (SliceIndex::FromStart(start), Some(SliceIndex::FromStart(end))) => {
                explain_resolved(start, Some(end), step, unit)
            }
            (SliceIndex::FromStart(start), Some(SliceIndex::FromEnd(back))) => {
                explain_lag(start, back, step, unit)
            }
            (SliceIndex::FromEnd(back), end) => explain_tail(back, end, step, unit),
        }
    }

    /// Render the nearest equivalent shell command(s) for this range and
    /// `mode`, without reading any input. Mirrors [`Self::explain`]: the same
    /// four `(start, end)` arms, classified once into a candidate per dialect.
    pub(crate) fn translate(&self, mode: TranslateMode, dialect: TranslateDialect) -> String {
        let plan = self.plan();
        let empty = matches!(plan, Plan::Resolved(SlicePlan::Empty));
        let copy = matches!(plan, Plan::Resolved(SlicePlan::Copy));
        let step = self.step.magnitude().get();
        let candidate = |d: Dialect| -> Result<Translation, &'static str> {
            if copy {
                Ok(("cat".to_owned(), None))
            } else if empty {
                empty_candidate(mode, d)
            } else {
                self.candidate(mode, step, d)
            }
        };
        match dialect {
            // `all` lists every dialect; a dialect with no equivalent gets a
            // terse `(no equivalent)` rather than the single-dialect reason line.
            TranslateDialect::All => {
                let mut out = String::new();
                for d in [Dialect::Posix, Dialect::Bsd, Dialect::Gnu, Dialect::Awk] {
                    let label = dialect_label(d);
                    match candidate(d) {
                        Ok((cmd, note)) => out.push_str(&render_block(label, &cmd, note)),
                        Err(_) => out.push_str(&format!("# {label}  (no equivalent)\n")),
                    }
                }
                out
            }
            _ => {
                let d = match dialect {
                    TranslateDialect::Posix => Dialect::Posix,
                    TranslateDialect::Bsd => Dialect::Bsd,
                    TranslateDialect::Gnu => Dialect::Gnu,
                    TranslateDialect::Awk => Dialect::Awk,
                    TranslateDialect::All => unreachable!(),
                };
                render_result(dialect_label(d), candidate(d))
            }
        }
    }

    /// The command for one concrete dialect, or `Err(reason)` when that dialect
    /// has no faithful single-command equivalent. `Empty`/`Copy` are handled by
    /// the caller, so the resolved arms here always select something.
    fn candidate(
        &self,
        mode: TranslateMode,
        step: usize,
        dialect: Dialect,
    ) -> Result<Translation, &'static str> {
        use SliceIndex::*;
        if matches!(self.step, Step::Backward(_)) {
            return translate_reverse(self.start, self.end, step, mode, dialect);
        }
        match (self.start, self.end) {
            (FromStart(start), None) => translate_unbounded(start, step, mode, dialect),
            (FromStart(start), Some(FromStart(end))) => {
                translate_bounded(start, end, step, mode, dialect)
            }
            (FromStart(start), Some(FromEnd(back))) => {
                translate_lag(start, back, step, mode, dialect)
            }
            (FromEnd(back), end) => translate_tail(back, end, step, mode, dialect),
        }
    }
}

fn explain_resolved(start: usize, end: Option<usize>, step: usize, unit: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("start: {start}\n"));
    match end {
        None => out.push_str("end:   end of input\n"),
        Some(end) => out.push_str(&format!("end:   {end} (exclusive)\n")),
    }
    out.push_str(&format!("step:  {step}\n"));

    match end {
        None => out.push_str(&format!(
            "0-based: {unit}s at indices [{start}, end of input)"
        )),
        Some(end) => out.push_str(&format!("0-based: {unit}s at indices [{start}, {end})")),
    }
    if step != 1 {
        out.push_str(&format!(", every {step} starting at {start}"));
    }
    out.push('\n');

    // 1-based human positions ("Nth line").
    let first_pos = start.saturating_add(1);
    match end {
        None => {
            if step == 1 {
                out.push_str(&format!(
                    "1-based: from the {} {unit} to the last {unit}\n",
                    ordinal(first_pos)
                ));
            } else {
                out.push_str(&format!(
                    "1-based: every {step}{} {unit} from the {} {unit} to the last {unit}\n",
                    ordinal_suffix(step),
                    ordinal(first_pos)
                ));
            }
            out.push_str(&format!("count: until end of input (step {step})"));
        }
        // end is exclusive 0-based, so the last selected 1-based position is `end`.
        Some(end) => {
            if start >= end {
                out.push_str(&format!(
                    "1-based: empty (start {first_pos} is at or past end {end})\n"
                ));
                out.push_str("count: 0");
            } else {
                if step == 1 {
                    out.push_str(&format!(
                        "1-based: from the {} {unit} to the {} {unit}\n",
                        ordinal(first_pos),
                        ordinal(end)
                    ));
                } else {
                    out.push_str(&format!(
                        "1-based: every {step}{} {unit} from the {} {unit} up to the {} {unit}\n",
                        ordinal_suffix(step),
                        ordinal(first_pos),
                        ordinal(end)
                    ));
                }
                let span = end - start;
                let count = span.div_ceil(step);
                out.push_str(&format!("count: {count}"));
            }
        }
    }
    out.push('\n');
    out
}

/// Tail-relative end: the resolved bound is `length - back`, unknowable
/// without reading the input, so positions are described symbolically.
fn explain_lag(start: usize, back: NonZeroUsize, step: usize, unit: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("start: {start}\n"));
    out.push_str(&format!("end:   {back} from the end (exclusive)\n"));
    out.push_str(&format!("step:  {step}\n"));

    out.push_str(&format!(
        "0-based: {unit}s at indices [{start}, length-{back})"
    ));
    if step != 1 {
        out.push_str(&format!(", every {step} starting at {start}"));
    }
    out.push_str(", clamped to the input length\n");

    let first_pos = start.saturating_add(1);
    // The last index the bound admits is length-back-1, i.e. the (back+1)-th
    // position counting back from the end.
    let last_from_end = back.get().saturating_add(1);
    if step == 1 {
        out.push_str(&format!(
            "1-based: from the {} {unit} to the {} {unit} from the end\n",
            ordinal(first_pos),
            ordinal(last_from_end)
        ));
    } else {
        out.push_str(&format!(
            "1-based: every {step}{} {unit} from the {} {unit} up to the {} {unit} from the end\n",
            ordinal_suffix(step),
            ordinal(first_pos),
            ordinal(last_from_end)
        ));
    }
    out.push_str("count: depends on the input length\n");
    out
}

/// Tail-relative start: the resolved offset is `length - back`, unknowable
/// without reading the input, so positions are described symbolically.
fn explain_tail(back: NonZeroUsize, end: Option<SliceIndex>, step: usize, unit: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("start: {back} from the end\n"));
    match end {
        None => out.push_str("end:   end of input\n"),
        Some(SliceIndex::FromStart(end)) => out.push_str(&format!("end:   {end} (exclusive)\n")),
        Some(SliceIndex::FromEnd(m)) => {
            out.push_str(&format!("end:   {m} from the end (exclusive)\n"))
        }
    }
    out.push_str(&format!("step:  {step}\n"));

    // The statically empty pairs ([L-k, L-m) with k <= m, and end 0) mirror
    // the resolved empty form: no clamping note, count 0.
    let static_empty_end = match end {
        Some(SliceIndex::FromEnd(m)) if back <= m => Some(format!("length-{m}")),
        Some(SliceIndex::FromStart(0)) => Some("0".to_owned()),
        _ => None,
    };
    if let Some(end) = static_empty_end {
        out.push_str(&format!(
            "0-based: {unit}s at indices [length-{back}, {end})\n"
        ));
        out.push_str(&format!(
            "1-based: empty (start length-{back} is at or past end {end})\n"
        ));
        out.push_str("count: 0\n");
        return out;
    }

    match end {
        None => out.push_str(&format!(
            "0-based: {unit}s at indices [length-{back}, end of input)"
        )),
        Some(SliceIndex::FromStart(end)) => out.push_str(&format!(
            "0-based: {unit}s at indices [length-{back}, {end})"
        )),
        Some(SliceIndex::FromEnd(m)) => out.push_str(&format!(
            "0-based: {unit}s at indices [length-{back}, length-{m})"
        )),
    }
    if step != 1 {
        out.push_str(&format!(", every {step} starting at length-{back}"));
    }
    out.push_str(", clamped to the input length\n");

    let first = format!("the {} {unit} from the end", ordinal(back.get()));
    let last = match end {
        None => format!("the last {unit}"),
        // end is exclusive 0-based, so the last selected 1-based position is `end`.
        Some(SliceIndex::FromStart(end)) => format!("the {} {unit}", ordinal(end)),
        // The last index the bound admits is length-m-1, i.e. the (m+1)-th
        // position counting back from the end.
        Some(SliceIndex::FromEnd(m)) => format!(
            "the {} {unit} from the end",
            ordinal(m.get().saturating_add(1))
        ),
    };
    if step == 1 {
        out.push_str(&format!("1-based: from {first} to {last}\n"));
    } else {
        out.push_str(&format!(
            "1-based: every {step}{} {unit} from {first} up to {last}\n",
            ordinal_suffix(step)
        ));
    }

    match end {
        // The window spans at most `back` (resp. `back - m`) positions.
        None => out.push_str(&format!("count: at most {}\n", back.get().div_ceil(step))),
        Some(SliceIndex::FromEnd(m)) => out.push_str(&format!(
            "count: at most {}\n",
            (back.get() - m.get()).div_ceil(step)
        )),
        Some(SliceIndex::FromStart(_)) => out.push_str("count: depends on the input length\n"),
    }
    out
}

/// A negative step: selection runs backward from `start` (default: the last
/// element) down to just above `end`. The wording mirrors the forward forms.
fn explain_reverse(start: SliceIndex, end: Option<SliceIndex>, step: usize, unit: &str) -> String {
    use SliceIndex::*;
    let mut out = String::new();
    match start {
        FromEnd(k) if k.get() == 1 => out.push_str(&format!("start: last {unit}\n")),
        FromEnd(k) => out.push_str(&format!("start: {k} from the end\n")),
        FromStart(s) => out.push_str(&format!("start: {s}\n")),
    }
    match end {
        None => out.push_str("end:   start of input\n"),
        Some(FromStart(e)) => out.push_str(&format!("end:   {e} (exclusive)\n")),
        Some(FromEnd(m)) => out.push_str(&format!("end:   {m} from the end (exclusive)\n")),
    }
    out.push_str(&format!("step:  -{step} (reverse)\n"));

    // Must stay in lockstep with the static-Empty arms in `plan()` so
    // --explain agrees with execution.
    let statically_empty = match (start, end) {
        (FromStart(s), Some(FromStart(e))) if s <= e => Some(format!(
            "0-based: none (start {s} is at or below end {e} for a reverse step)\n"
        )),
        (FromEnd(k), Some(FromEnd(m))) if k >= m => Some(format!(
            "0-based: none (start {k} from the end is at or below end {m} from the end for a reverse step)\n"
        )),
        (_, Some(FromEnd(m))) if m.get() == 1 => Some(format!(
            "0-based: none (an end 1 from the end excludes every {unit} a reverse step can reach)\n"
        )),
        _ => None,
    };
    if let Some(zero_based) = statically_empty {
        out.push_str(&zero_based);
        out.push_str("1-based: empty\n");
        out.push_str("count: 0\n");
        return out;
    }

    // 1-based endpoints of the walk, spelled from the parsed form.
    let from = match start {
        FromEnd(k) if k.get() == 1 => format!("the last {unit}"),
        FromEnd(k) => format!("the {} {unit} from the end", ordinal(k.get())),
        FromStart(s) => format!("the {} {unit}", ordinal(s.saturating_add(1))),
    };
    let to = match end {
        None => format!("the first {unit}"),
        Some(FromStart(e)) => format!("the {} {unit}", ordinal(e.saturating_add(2))),
        Some(FromEnd(m)) => format!(
            "the {} {unit} from the end",
            ordinal(m.get().saturating_add(1))
        ),
    };

    match start {
        // Default-shaped start: the walk covers the length-anchored span
        // `[lower, length-(k-1))`.
        FromEnd(k) => {
            let upper = match k.get() {
                1 => "length".to_owned(),
                k => format!("length-{}", k - 1),
            };
            let lower = match end {
                None => "0".to_owned(),
                Some(FromStart(e)) => format!("{}", e.saturating_add(1)),
                Some(FromEnd(m)) => format!("length-{}", m.get() - 1),
            };
            out.push_str(&format!(
                "0-based: {unit}s at indices [{lower}, {upper}) in reverse order"
            ));
            if step != 1 {
                match k.get() {
                    1 => out.push_str(&format!(", every {step} starting at the last")),
                    k => out.push_str(&format!(", every {step} starting at {k} from the end")),
                }
            }
            out.push('\n');
        }
        // Explicit start: the walk is a clamped descent from a fixed index.
        FromStart(s) => {
            let lower = match end {
                None => "0".to_owned(),
                Some(FromStart(e)) => format!("{} (end {e} exclusive)", e.saturating_add(1)),
                Some(FromEnd(m)) => {
                    let m = m.get();
                    format!("length-{} (end length-{m} exclusive)", m - 1)
                }
            };
            out.push_str(&format!("0-based: {unit}s at indices {s} down to {lower}"));
            if step != 1 {
                out.push_str(&format!(", every {step}"));
            }
            out.push_str(", clamped to the input length\n");
        }
    }

    if step == 1 {
        out.push_str(&format!("1-based: from {from} to {to}\n"));
    } else {
        out.push_str(&format!(
            "1-based: every {step}{} {unit} from {from} to {to}\n",
            ordinal_suffix(step)
        ));
    }

    match (start, end) {
        (FromEnd(_), None) => {
            if step == 1 {
                out.push_str("count: until end of input (reverse)\n");
            } else {
                out.push_str(&format!(
                    "count: until end of input (reverse, step {step})\n"
                ));
            }
        }
        (FromEnd(_), Some(FromStart(e))) => {
            let excluded = match e.saturating_add(1) {
                1 => format!("the first {unit}"),
                n => format!("the first {n} {unit}s"),
            };
            if step == 1 {
                out.push_str(&format!(
                    "count: until end of input (reverse), excluding {excluded}\n"
                ));
            } else {
                out.push_str(&format!(
                    "count: until end of input (reverse, step {step}), excluding {excluded}\n"
                ));
            }
        }
        // The walk spans at most `m - k` positions.
        (FromEnd(k), Some(FromEnd(m))) => out.push_str(&format!(
            "count: at most {}\n",
            (m.get() - k.get()).div_ceil(step)
        )),
        // s down to 0 inclusive.
        (FromStart(s), None) => out.push_str(&format!("count: at most {}\n", s / step + 1)),
        (FromStart(s), Some(FromStart(e))) => {
            out.push_str(&format!("count: at most {}\n", (s - e).div_ceil(step)))
        }
        (FromStart(_), Some(FromEnd(_))) => out.push_str("count: depends on the input length\n"),
    }
    out
}

/// A rendered translation: the shell command and an optional caveat shown as
/// an inline note next to the dialect-labelled comment line.
type Translation = (String, Option<&'static str>);

const CUSTOM_REASON: &str = "no standard tool selects records by a custom delimiter";
const CHARS_REASON: &str =
    "cut -c is per-line (and selects bytes on GNU), not a stream character slice";
const GRAPHEMES_REASON: &str = "no standard tool selects by grapheme cluster";
const STEP_BYTE_REASON: &str = "strided byte selection has no standard-tool equivalent";
const AWK_BYTE_REASON: &str = "awk operates on lines, not byte offsets";
const AWK_NEWLINE_NOTE: &str =
    "awk re-terminates an unterminated final line and may drop bytes after an embedded NUL";
const AWK_TAIL_REASON: &str =
    "selecting relative to the end needs buffering, not a clean awk one-liner";
const LAG_START_REASON: &str =
    "a head-relative start with a tail-relative end needs the input length";
const LAG_STEP_REASON: &str =
    "a tail-relative end combined with a step has no standard-tool equivalent";
const DROP_LAST_LINES_REASON: &str =
    "no POSIX single command drops the last N lines; GNU has `head -n -N`";
const DROP_LAST_BYTES_REASON: &str =
    "no POSIX single command drops the last N bytes; GNU has `head -c -N`";
const EMPTY_RANGE_LINES_REASON: &str =
    "no POSIX single command selects an empty range; GNU has `head -n 0`";
const EMPTY_RANGE_BYTES_REASON: &str =
    "no POSIX single command selects an empty range; GNU has `head -c 0`";
const TAIL_STEP_REASON: &str =
    "a tail-relative start combined with a step has no standard-tool equivalent";
const TAIL_BOUNDED_REASON: &str = "a tail-relative start with a fixed end needs the input length";
const HEAD_C_NOTE: &str = "head -c is not in POSIX; present on BSD and GNU";
const REVERSE_POSIX_REASON: &str =
    "no POSIX single command reverses line order; GNU has tac, BSD has tail -r";
const REVERSE_PARTIAL_REASON: &str = "reversing a sub-range needs a pipeline, not a single command";
const REVERSE_STEP_REASON: &str = "a reverse with a step needs a pipeline, not a single command";
const REVERSE_BYTES_REASON: &str = "no standard tool reverses a byte stream";

fn dialect_label(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Posix => "posix",
        Dialect::Bsd => "bsd",
        Dialect::Gnu => "gnu",
        Dialect::Awk => "awk",
    }
}

/// A dialect-labelled block: a comment line naming the dialect (with an
/// optional inline note) followed by the command line, so the result pastes
/// straight into a shell.
fn render_block(label: &str, cmd: &str, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("# {label}  ({note})\n{cmd}\n"),
        None => format!("# {label}\n{cmd}\n"),
    }
}

fn render_untranslatable(reason: &str) -> String {
    format!("# no equivalent: {reason}\n")
}

fn render_result(label: &str, candidate: Result<Translation, &'static str>) -> String {
    match candidate {
        Ok((cmd, note)) => render_block(label, &cmd, note),
        Err(reason) => render_untranslatable(reason),
    }
}

/// The empty range selects nothing. Short-circuited ahead of the per-mode
/// candidate, so it carries its own mode split: only GNU `head` accepts a zero
/// count (`head -n 0` for lines, `head -c 0` for bytes); POSIX/BSD `head` reject
/// it, so no portable single command expresses it. A custom delimiter has no
/// standard-tool equivalent even when empty, matching every other custom range.
///
/// A GNU-only spelling added here must also be taught to xtask's `classify_tier`
/// (xtask/src/check.rs), which re-derives the tier from the command's shape.
fn empty_candidate(mode: TranslateMode, dialect: Dialect) -> Result<Translation, &'static str> {
    match mode {
        TranslateMode::Custom => Err(CUSTOM_REASON),
        TranslateMode::Chars => Err(CHARS_REASON),
        TranslateMode::Graphemes => Err(GRAPHEMES_REASON),
        TranslateMode::Lines => match dialect {
            Dialect::Gnu => Ok(("head -n 0".to_owned(), None)),
            _ => Err(EMPTY_RANGE_LINES_REASON),
        },
        TranslateMode::Bytes => match dialect {
            Dialect::Gnu => Ok(("head -c 0".to_owned(), None)),
            _ => Err(EMPTY_RANGE_BYTES_REASON),
        },
    }
}

/// Build the awk condition for a stepped line selection. `first` is the 1-based
/// first row; `end` is the inclusive 1-based last row when bounded.
fn awk_stepped(first: usize, step: usize, end: Option<usize>) -> String {
    let mut cond = if first == 1 {
        format!("NR%{step}==1")
    } else {
        format!("NR>={first} && (NR-{first})%{step}==0")
    };
    if let Some(end) = end {
        cond = format!("{cond} && NR<={end}");
    }
    format!("awk '{cond}'")
}

/// `start:` — unbounded from a head-relative start. `start == 0 && step == 1`
/// is the Copy fast path, handled before this is reached.
fn translate_unbounded(
    start: usize,
    step: usize,
    mode: TranslateMode,
    dialect: Dialect,
) -> Result<Translation, &'static str> {
    match mode {
        TranslateMode::Custom => return Err(CUSTOM_REASON),
        TranslateMode::Chars => return Err(CHARS_REASON),
        TranslateMode::Graphemes => return Err(GRAPHEMES_REASON),
        TranslateMode::Bytes if step > 1 => return Err(STEP_BYTE_REASON),
        _ => {}
    }
    // Saturating, like the explain sites: `start` can be usize::MAX (the range
    // `18446744073709551615:` parses to FromStart(MAX)) and a plain `+ 1` would
    // overflow-panic in debug and silently wrap in release.
    let first = start.saturating_add(1); // 1-based
    if step == 1 {
        match mode {
            TranslateMode::Lines => match dialect {
                Dialect::Awk => Ok((format!("awk 'NR>={first}'"), Some(AWK_NEWLINE_NOTE))),
                _ => Ok((format!("tail -n +{first}"), None)),
            },
            TranslateMode::Bytes => match dialect {
                Dialect::Awk => Err(AWK_BYTE_REASON),
                _ => Ok((format!("tail -c +{first}"), None)),
            },
            TranslateMode::Chars | TranslateMode::Graphemes | TranslateMode::Custom => {
                unreachable!()
            }
        }
    } else {
        // Unbounded stepped: lines only (byte and custom errored above).
        match dialect {
            Dialect::Gnu => Ok((format!("sed -n '{first}~{step}p'"), None)),
            _ => Ok((awk_stepped(first, step, None), Some(AWK_NEWLINE_NOTE))),
        }
    }
}

/// `start:end` — bounded window or bounded stepped. `start < end` always (the
/// empty case is handled before this is reached); `end` is the inclusive
/// 1-based last row, since the 0-based exclusive end equals it.
fn translate_bounded(
    start: usize,
    end: usize,
    step: usize,
    mode: TranslateMode,
    dialect: Dialect,
) -> Result<Translation, &'static str> {
    if mode == TranslateMode::Custom {
        return Err(CUSTOM_REASON);
    }
    if mode == TranslateMode::Chars {
        return Err(CHARS_REASON);
    }
    if mode == TranslateMode::Graphemes {
        return Err(GRAPHEMES_REASON);
    }
    // A byte step matters only if a second selected byte falls within
    // [start, end); when it does not (`end - start <= step`), the range picks
    // the single byte at `start` — identical to the unstepped one-byte window —
    // so collapse it onto the dd/head -c path. A step that does reach a second
    // byte is a genuine stride with no standard-tool equivalent.
    let (end, step) = if mode == TranslateMode::Bytes && step > 1 {
        if end - start > step {
            return Err(STEP_BYTE_REASON);
        }
        (start.saturating_add(1), 1)
    } else {
        (end, step)
    };
    let first = start.saturating_add(1); // 1-based; saturates for parity with translate_unbounded
    if step > 1 {
        // GNU sed has no clean bounded `~` form, so every dialect uses awk.
        return Ok((awk_stepped(first, step, Some(end)), Some(AWK_NEWLINE_NOTE)));
    }
    match mode {
        TranslateMode::Lines => match dialect {
            Dialect::Awk => {
                let cond = if first == 1 {
                    format!("NR<={end}")
                } else {
                    format!("NR>={first} && NR<={end}")
                };
                Ok((format!("awk '{cond}'"), Some(AWK_NEWLINE_NOTE)))
            }
            _ if start == 0 => Ok((format!("head -n {end}"), None)),
            _ if end == first => Ok((format!("sed -n '{first}p'"), None)),
            _ => Ok((format!("sed -n '{first},{end}p'"), None)),
        },
        TranslateMode::Bytes => {
            let count = end - start;
            match dialect {
                Dialect::Awk => Err(AWK_BYTE_REASON),
                Dialect::Bsd | Dialect::Gnu if start == 0 => {
                    Ok((format!("head -c {end}"), Some(HEAD_C_NOTE)))
                }
                _ if start == 0 => Ok((format!("dd bs=1 count={count} 2>/dev/null"), None)),
                _ => Ok((
                    format!("dd bs=1 skip={start} count={count} 2>/dev/null"),
                    None,
                )),
            }
        }
        TranslateMode::Chars | TranslateMode::Graphemes | TranslateMode::Custom => unreachable!(),
    }
}

/// `start:-back` — head-relative start, tail-relative end ("drop the last
/// `back`"). Only `start == 0 && step == 1` is statically expressible.
fn translate_lag(
    start: usize,
    back: NonZeroUsize,
    step: usize,
    mode: TranslateMode,
    dialect: Dialect,
) -> Result<Translation, &'static str> {
    if mode == TranslateMode::Custom {
        return Err(CUSTOM_REASON);
    }
    if mode == TranslateMode::Chars {
        return Err(CHARS_REASON);
    }
    if mode == TranslateMode::Graphemes {
        return Err(GRAPHEMES_REASON);
    }
    if start > 0 {
        return Err(LAG_START_REASON);
    }
    if step > 1 {
        return Err(LAG_STEP_REASON);
    }
    let back = back.get();
    match mode {
        TranslateMode::Lines => match dialect {
            Dialect::Awk => Err(AWK_TAIL_REASON),
            Dialect::Gnu => Ok((format!("head -n -{back}"), None)),
            // POSIX has a single-command form only for the last line.
            _ if back == 1 => Ok(("sed '$d'".to_owned(), None)),
            _ => Err(DROP_LAST_LINES_REASON),
        },
        TranslateMode::Bytes => match dialect {
            Dialect::Awk => Err(AWK_BYTE_REASON),
            Dialect::Gnu => Ok((format!("head -c -{back}"), None)),
            _ => Err(DROP_LAST_BYTES_REASON),
        },
        TranslateMode::Chars | TranslateMode::Graphemes | TranslateMode::Custom => unreachable!(),
    }
}

/// A negative step. Only the full line reverse (`::-1`) has single-command
/// equivalents — GNU `tac` and BSD `tail -r`; any bound or stride needs a
/// pipeline, and no standard tool reverses a byte stream. The
/// statically-empty reverse forms are handled by `empty_candidate` before
/// this is reached.
fn translate_reverse(
    start: SliceIndex,
    end: Option<SliceIndex>,
    step: usize,
    mode: TranslateMode,
    dialect: Dialect,
) -> Result<Translation, &'static str> {
    match mode {
        TranslateMode::Custom => return Err(CUSTOM_REASON),
        TranslateMode::Chars => return Err(CHARS_REASON),
        TranslateMode::Graphemes => return Err(GRAPHEMES_REASON),
        TranslateMode::Bytes => return Err(REVERSE_BYTES_REASON),
        TranslateMode::Lines => {}
    }
    if start != SliceIndex::FromEnd(NonZeroUsize::MIN) || end.is_some() {
        return Err(REVERSE_PARTIAL_REASON);
    }
    if step > 1 {
        return Err(REVERSE_STEP_REASON);
    }
    match dialect {
        Dialect::Gnu => Ok(("tac".to_owned(), None)),
        Dialect::Bsd => Ok(("tail -r".to_owned(), None)),
        Dialect::Posix => Err(REVERSE_POSIX_REASON),
        Dialect::Awk => Err(AWK_TAIL_REASON),
    }
}

/// `-back:…` — tail-relative start. Only `-back:` (unbounded) is statically
/// expressible; any fixed or tail-relative end needs the input length. The
/// statically-empty ends (`-back:0`, `-k:-m` with k<=m) are handled before this
/// is reached.
fn translate_tail(
    back: NonZeroUsize,
    end: Option<SliceIndex>,
    step: usize,
    mode: TranslateMode,
    dialect: Dialect,
) -> Result<Translation, &'static str> {
    if mode == TranslateMode::Custom {
        return Err(CUSTOM_REASON);
    }
    if mode == TranslateMode::Chars {
        return Err(CHARS_REASON);
    }
    if mode == TranslateMode::Graphemes {
        return Err(GRAPHEMES_REASON);
    }
    if step > 1 {
        return Err(TAIL_STEP_REASON);
    }
    if end.is_some() {
        return Err(TAIL_BOUNDED_REASON);
    }
    let back = back.get();
    match mode {
        TranslateMode::Lines => match dialect {
            Dialect::Awk => Err(AWK_TAIL_REASON),
            _ => Ok((format!("tail -n {back}"), None)),
        },
        TranslateMode::Bytes => match dialect {
            Dialect::Awk => Err(AWK_BYTE_REASON),
            _ => Ok((format!("tail -c {back}"), None)),
        },
        TranslateMode::Chars | TranslateMode::Graphemes | TranslateMode::Custom => unreachable!(),
    }
}

/// "1st", "2nd", "3rd", "4th" ...
fn ordinal(n: usize) -> String {
    format!("{n}{}", ordinal_suffix(n))
}

fn ordinal_suffix(n: usize) -> &'static str {
    match (n % 100, n % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    }
}

/// The digit string after a leading `-`, when the lexeme is sign-then-digits
/// (`-12`); `None` for anything else (`-`, `--1`, `-+1`).
fn digit_shaped_magnitude(s: &str) -> Option<&str> {
    s.strip_prefix('-')
        .filter(|magnitude| magnitude.as_bytes().first().is_some_and(u8::is_ascii_digit))
}

/// Parse one bound of the range. A leading `-` followed by a bare digit string
/// is tail-relative; anything else (`-`, `--1`, `-+1`) keeps the plain-integer
/// parse error so rejection messages stay unchanged, while a digit-shaped
/// magnitude reports its own failure (overflow) instead of blaming the `-`.
fn parse_index(s: &str, field: RangeField) -> Result<Option<SliceIndex>, ParseSliceRangeError> {
    match s.parse::<usize>() {
        Ok(v) => Ok(Some(SliceIndex::FromStart(v))),
        Err(err) if *err.kind() == IntErrorKind::Empty => Ok(None),
        Err(source) => {
            let source = match digit_shaped_magnitude(s) {
                Some(magnitude) => match magnitude.parse::<usize>() {
                    Ok(v) => {
                        return Ok(Some(match NonZeroUsize::new(v) {
                            Some(back) => SliceIndex::FromEnd(back),
                            // Python has no -0: it means the head, not the end.
                            None => SliceIndex::FromStart(0),
                        }));
                    }
                    Err(inner) => inner,
                },
                None => source,
            };
            Err(ParseSliceRangeError::InvalidField {
                field,
                value: s.to_owned(),
                source,
            })
        }
    }
}

/// Parse the step field. A leading `-` followed by a bare nonzero digit
/// string is a reverse step; a bare `-` and signed non-digits keep the
/// plain-integer parse error, so rejection messages stay unchanged, while a
/// digit-shaped magnitude reports its own failure (zero, overflow) instead of
/// blaming the `-`.
fn parse_step(s: &str) -> Result<Option<Step>, ParseSliceRangeError> {
    match s.parse::<NonZeroUsize>() {
        Ok(step) => Ok(Some(Step::Forward(step))),
        Err(err) if *err.kind() == IntErrorKind::Empty => Ok(None),
        Err(source) => {
            let source = match digit_shaped_magnitude(s) {
                Some(magnitude) => match magnitude.parse::<NonZeroUsize>() {
                    Ok(step) => return Ok(Some(Step::Backward(step))),
                    Err(inner) => inner,
                },
                None => source,
            };
            Err(ParseSliceRangeError::InvalidField {
                field: RangeField::Step,
                value: s.to_owned(),
                source,
            })
        }
    }
}

/// Relative end for a tail-relative start: `end = (k from the end) - n`.
/// Reaching or passing the end of input unbounds it (`min(L, ·) = L` is
/// exactly what `end = None` selects).
#[inline]
fn end_minus(k: NonZeroUsize, n: usize) -> Option<SliceIndex> {
    NonZeroUsize::new(k.get().saturating_sub(n)).map(SliceIndex::FromEnd)
}

impl FromStr for SliceRange {
    type Err = ParseSliceRangeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn parse<T: FromStr<Err = ParseIntError>>(
            s: &str,
            field: RangeField,
        ) -> Result<Option<T>, ParseSliceRangeError> {
            match s.parse::<T>() {
                Ok(v) => Ok(Some(v)),
                Err(err) if *err.kind() == IntErrorKind::Empty => Ok(None),
                Err(source) => Err(ParseSliceRangeError::InvalidField {
                    field,
                    value: s.to_owned(),
                    source,
                }),
            }
        }
        let relative_amount = |amount: &str| -> Result<usize, ParseSliceRangeError> {
            parse(amount, RangeField::End)?.ok_or(ParseSliceRangeError::MissingRelativeAmount)
        };

        /// The end field's parsed form, held until the step is known: the
        /// relative forms resolve against the start for a forward step and are
        /// rejected for a reverse one.
        enum End {
            Plain(Option<SliceIndex>),
            Ahead(usize),
            Window(usize),
        }

        let mut ptn = s.split(':');
        let start = parse_index(ptn.next().unwrap_or(""), RangeField::Start)?;
        let maybe_end = ptn.next().ok_or(ParseSliceRangeError::MissingColon)?;
        // Parse the end before the step so field errors keep reporting left
        // to right.
        let end = if let Some(amount) = maybe_end.strip_prefix("+-") {
            End::Window(relative_amount(amount)?)
        } else if let Some(amount) = maybe_end.strip_prefix('+') {
            End::Ahead(relative_amount(amount)?)
        } else {
            End::Plain(parse_index(maybe_end, RangeField::End)?)
        };
        let step = match ptn.next() {
            Some(step) => parse_step(step)?,
            None => None,
        }
        .unwrap_or(Step::Forward(NonZeroUsize::MIN));
        if ptn.next().is_some() {
            return Err(ParseSliceRangeError::TooManyParts);
        }
        let reverse = matches!(step, Step::Backward(_));
        let (start, end) = match end {
            End::Plain(end) => {
                // A reverse walk starts from the last element by default.
                let default = if reverse {
                    SliceIndex::FromEnd(NonZeroUsize::MIN)
                } else {
                    SliceIndex::FromStart(0)
                };
                (start.unwrap_or(default), end)
            }
            End::Ahead(_) | End::Window(_) if reverse => {
                return Err(ParseSliceRangeError::NegativeStepWithRelativeEnd)
            }
            End::Ahead(lines) => {
                let start = start.unwrap_or(SliceIndex::FromStart(0));
                match start {
                    SliceIndex::FromStart(s) => {
                        (start, Some(SliceIndex::FromStart(s.saturating_add(lines))))
                    }
                    SliceIndex::FromEnd(k) => (start, end_minus(k, lines)),
                }
            }
            End::Window(lines) => match start.unwrap_or(SliceIndex::FromStart(0)) {
                SliceIndex::FromStart(start) => (
                    SliceIndex::FromStart(start.saturating_sub(lines)),
                    Some(SliceIndex::FromStart(start.saturating_add(lines))),
                ),
                // The window [start-n, start+n) in tail coordinates: the back
                // distance grows by n, the end distance shrinks by n.
                SliceIndex::FromEnd(k) => (
                    SliceIndex::FromEnd(k.saturating_add(lines)),
                    end_minus(k, lines),
                ),
            },
        };
        Ok(Self { start, end, step })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let slice = SliceRange::from_str("0:1:1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: Some(SliceIndex::FromStart(1)),
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn without_step() {
        let slice = SliceRange::from_str("0:1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: Some(SliceIndex::FromStart(1)),
                step: Step::forward(1),
            }
        );
        let slice = SliceRange::from_str("0:1:").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: Some(SliceIndex::FromStart(1)),
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn without_start() {
        let slice = SliceRange::from_str(":1:1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: Some(SliceIndex::FromStart(1)),
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn without_end() {
        let slice = SliceRange::from_str("0::1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: None,
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn without_start_and_end() {
        let slice = SliceRange::from_str("::1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: None,
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn without_all() {
        let slice = SliceRange::from_str(":").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: None,
                step: Step::forward(1),
            }
        );
        let slice = SliceRange::from_str("::").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: None,
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn default_step_is_one() {
        let slice = SliceRange::from_str("0:1:").expect("parse failed.");
        assert_eq!(slice.step, Step::Forward(NonZeroUsize::MIN));
    }

    #[test]
    fn plan_copies_whole_input_ranges() {
        for whole in [":", "::", "0:", "0::", "::1", "0::1", "-0:"] {
            assert_eq!(
                SliceRange::from_str(whole).unwrap().plan(),
                Plan::Resolved(SlicePlan::Copy),
                "{whole} should select the whole input"
            );
        }
    }

    #[test]
    fn plan_windows_contiguous_subranges() {
        for (range, start, end) in [
            ("1:", 1, None),
            (":1", 0, Some(1)),
            ("5:15", 5, Some(15)),
            ("1::1", 1, None),
            ("1:+3", 1, Some(4)),
            ("5:+-10", 0, Some(15)),
        ] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Window { start, end }),
                "{range} should be a contiguous window"
            );
        }
    }

    #[test]
    fn plan_steps_non_contiguous_selections() {
        for (range, start, end, step) in [("::2", 0, None, 2), ("1:8:2", 1, Some(8), 2)] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Stepped {
                    start,
                    end,
                    step: NonZeroUsize::new(step).unwrap()
                }),
                "{range} must stay on the stepped pipeline"
            );
        }
    }

    #[test]
    fn plan_empties_bounded_start_at_or_past_end() {
        for empty in ["5:3", "5:5", ":0", "5:3:2", "5:+0", ":-0"] {
            assert_eq!(
                SliceRange::from_str(empty).unwrap().plan(),
                Plan::Resolved(SlicePlan::Empty),
                "{empty} selects nothing and must not read input"
            );
        }
    }

    #[test]
    fn plan_keeps_nonempty_bounded_ranges_off_empty() {
        for (range, start, end) in [("0:1", 0, Some(1)), ("4:5", 4, Some(5))] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Window { start, end }),
                "{range} selects something and must stay a window"
            );
        }
    }

    #[test]
    fn plus_sign() {
        let slice = SliceRange::from_str("1:+1").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(1),
                end: Some(SliceIndex::FromStart(2)),
                step: Step::forward(1),
            }
        )
    }

    #[test]
    fn plus_minus_sign() {
        let slice = SliceRange::from_str("100:+-10").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(90),
                end: Some(SliceIndex::FromStart(110)),
                step: Step::forward(1),
            }
        )
    }

    #[test]
    fn plus_minus_sign_saturates_start() {
        let slice = SliceRange::from_str("5:+-10").expect("parse failed.");
        assert_eq!(
            slice,
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: Some(SliceIndex::FromStart(15)),
                step: Step::forward(1),
            }
        )
    }

    #[test]
    fn negative_end() {
        assert_eq!(
            SliceRange::from_str(":-2").unwrap(),
            SliceRange {
                start: SliceIndex::FromStart(0),
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(2).unwrap())),
                step: Step::forward(1),
            }
        );
        assert_eq!(
            SliceRange::from_str("5:-3:2").unwrap(),
            SliceRange {
                start: SliceIndex::FromStart(5),
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(3).unwrap())),
                step: Step::forward(2),
            }
        );
        assert_eq!(
            SliceRange::from_str(":-18446744073709551615").unwrap().end,
            Some(SliceIndex::FromEnd(NonZeroUsize::new(usize::MAX).unwrap()))
        );
    }

    #[test]
    fn negative_start() {
        assert_eq!(
            SliceRange::from_str("-5:").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(5).unwrap()),
                end: None,
                step: Step::forward(1),
            }
        );
        assert_eq!(
            SliceRange::from_str("-4::2").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(4).unwrap()),
                end: None,
                step: Step::forward(2),
            }
        );
        assert_eq!(
            SliceRange::from_str("-7:8").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(7).unwrap()),
                end: Some(SliceIndex::FromStart(8)),
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn negative_both() {
        assert_eq!(
            SliceRange::from_str("-7:-2").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(7).unwrap()),
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(2).unwrap())),
                step: Step::forward(1),
            }
        );
        assert_eq!(
            SliceRange::from_str("-4:-1:2").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(4).unwrap()),
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(1).unwrap())),
                step: Step::forward(2),
            }
        );
    }

    #[test]
    fn max_magnitude_both_signs() {
        assert_eq!(
            SliceRange::from_str("18446744073709551615:").unwrap().start,
            SliceIndex::FromStart(usize::MAX)
        );
        assert_eq!(
            SliceRange::from_str("-18446744073709551615:")
                .unwrap()
                .start,
            SliceIndex::FromEnd(NonZeroUsize::new(usize::MAX).unwrap())
        );
    }

    #[test]
    fn minus_zero_normalizes_to_head() {
        assert_eq!(
            SliceRange::from_str(":-0").unwrap().end,
            Some(SliceIndex::FromStart(0))
        );
        assert_eq!(
            SliceRange::from_str("-0:").unwrap().start,
            SliceIndex::FromStart(0)
        );
    }

    #[test]
    fn plan_defers_tail_relative_end() {
        for (range, start, back, step) in [(":-2", 0, 2, 1), ("1:-1", 1, 1, 1), ("5:-3:2", 5, 3, 2)]
        {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Deferred(DeferredPlan::Lag {
                    start,
                    back: NonZeroUsize::new(back).unwrap(),
                    step: NonZeroUsize::new(step).unwrap(),
                }),
                "{range} needs the input length and must defer"
            );
        }
    }

    #[test]
    fn plan_defers_tail_relative_start() {
        for (range, back, end, step) in [
            ("-4:", 4, None, 1),
            ("-4::2", 4, None, 2),
            ("-7:8", 7, Some(SliceIndex::FromStart(8)), 1),
            (
                "-7:-2",
                7,
                Some(SliceIndex::FromEnd(NonZeroUsize::new(2).unwrap())),
                1,
            ),
            (
                "-7:-2:3",
                7,
                Some(SliceIndex::FromEnd(NonZeroUsize::new(2).unwrap())),
                3,
            ),
        ] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Deferred(DeferredPlan::Tail {
                    back: NonZeroUsize::new(back).unwrap(),
                    end,
                    step: NonZeroUsize::new(step).unwrap(),
                }),
                "{range} needs the input length and must defer"
            );
        }
    }

    #[test]
    fn plan_empties_negative_pairs_statically() {
        for empty in ["-2:-5", "-3:-3", "-5:0", "-5:+0"] {
            assert_eq!(
                SliceRange::from_str(empty).unwrap().plan(),
                Plan::Resolved(SlicePlan::Empty),
                "{empty} selects nothing for every length and must not read input"
            );
        }
    }

    fn deferred(range: &str) -> DeferredPlan {
        match SliceRange::from_str(range).unwrap().plan() {
            Plan::Deferred(deferred) => deferred,
            other => panic!("{range} must defer, planned {other:?}"),
        }
    }

    #[test]
    fn resolve_matches_python_indices() {
        fn bound(v: i64) -> SliceIndex {
            match NonZeroUsize::new(v.unsigned_abs() as usize) {
                Some(back) if v < 0 => SliceIndex::FromEnd(back),
                _ => SliceIndex::FromStart(v as usize),
            }
        }
        // Python slice(start, end, step).indices(len) for a positive step.
        fn python_indices(
            start: Option<i64>,
            end: Option<i64>,
            step: usize,
            len: usize,
        ) -> Vec<usize> {
            let len = len as i64;
            let clamp = |v: i64| if v < 0 { (len + v).max(0) } else { v.min(len) };
            (start.map_or(0, clamp)..end.map_or(len, clamp))
                .step_by(step)
                .map(|i| i as usize)
                .collect()
        }
        fn selected(plan: SlicePlan, len: usize) -> Vec<usize> {
            match plan {
                SlicePlan::Empty => Vec::new(),
                SlicePlan::Copy => (0..len).collect(),
                SlicePlan::Window { start, end } => {
                    (start..end.map_or(len, |end| end.min(len))).collect()
                }
                SlicePlan::Stepped { start, end, step } => (start
                    ..end.map_or(len, |end| end.min(len)))
                    .step_by(step.get())
                    .collect(),
            }
        }

        let bounds: Vec<Option<i64>> = std::iter::once(None).chain((-20..=20).map(Some)).collect();
        for len in 0..=40usize {
            for &start in &bounds {
                for &end in &bounds {
                    for step in [1usize, 2, 3, 7] {
                        let range = SliceRange {
                            start: start.map_or(SliceIndex::FromStart(0), bound),
                            end: end.map(bound),
                            step: Step::forward(step),
                        };
                        let plan = match range.plan() {
                            Plan::Resolved(plan) => plan,
                            Plan::Deferred(deferred) => deferred
                                .resolve(len as u64)
                                .expect("offsets fit usize on this platform"),
                            Plan::Reverse(reverse) => {
                                panic!("forward range planned {reverse:?}")
                            }
                        };
                        assert_eq!(
                            selected(plan, len),
                            python_indices(start, end, step, len),
                            "len={len} start={start:?} end={end:?} step={step}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn plan_classifies_reverse() {
        for range in ["::-1", "5:1:-1", "::-2", ":0:-1", "0::-1", "-1:-5:-1"] {
            assert!(
                matches!(
                    SliceRange::from_str(range).unwrap().plan(),
                    Plan::Reverse(_)
                ),
                "{range} must classify as a reverse plan"
            );
        }
        // Statically empty for every length: no input read at all.
        for range in ["1:5:-1", "5:5:-1", "-5:-2:-1", "-3:-3:-1", "9:-1:-1"] {
            assert_eq!(
                SliceRange::from_str(range).unwrap().plan(),
                Plan::Resolved(SlicePlan::Empty),
                "{range} selects nothing for every length"
            );
        }
    }

    #[test]
    fn reverse_indices_match_python() {
        fn bound(v: i64) -> SliceIndex {
            match NonZeroUsize::new(v.unsigned_abs() as usize) {
                Some(back) if v < 0 => SliceIndex::FromEnd(back),
                _ => SliceIndex::FromStart(v as usize),
            }
        }
        // Python slice(start, end, -step).indices(len): bounds clamp to
        // [-1, len-1], the walk descends while the index stays above stop.
        fn python_reverse_indices(
            start: Option<i64>,
            end: Option<i64>,
            step: usize,
            len: usize,
        ) -> Vec<usize> {
            let len = len as i64;
            let clamp = |v: i64| {
                if v < 0 {
                    (len + v).max(-1)
                } else {
                    v.min(len - 1)
                }
            };
            let first = start.map_or(len - 1, clamp);
            let stop = end.map_or(-1, clamp);
            let mut selected = Vec::new();
            let mut i = first;
            while i > stop {
                selected.push(i as usize);
                i -= step as i64;
            }
            selected
        }

        let bounds: Vec<Option<i64>> = std::iter::once(None).chain((-20..=20).map(Some)).collect();
        for len in 0..=40usize {
            for &start in &bounds {
                for &end in &bounds {
                    for step in [1usize, 2, 3, 7] {
                        let range = SliceRange {
                            // The parse defaults an absent reverse start to
                            // the last element.
                            start: start.map_or(SliceIndex::FromEnd(NonZeroUsize::MIN), bound),
                            end: end.map(bound),
                            step: Step::Backward(NonZeroUsize::new(step).unwrap()),
                        };
                        let selected: Vec<usize> = match range.plan() {
                            Plan::Resolved(SlicePlan::Empty) => Vec::new(),
                            Plan::Reverse(reverse) => reverse.indices(len).collect(),
                            other => panic!("reverse range planned {other:?}"),
                        };
                        assert_eq!(
                            selected,
                            python_reverse_indices(start, end, step, len),
                            "len={len} start={start:?} end={end:?} step=-{step}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn resolve_demotes_full_tail_to_copy() {
        assert_eq!(deferred("-100:").resolve(10), Some(SlicePlan::Copy));
        assert_eq!(deferred("-10:").resolve(10), Some(SlicePlan::Copy));
    }

    #[test]
    fn resolve_end_at_len_unbounds() {
        for range in ["-5:10", "-5:100"] {
            assert_eq!(
                deferred(range).resolve(10),
                Some(SlicePlan::Window {
                    start: 5,
                    end: None
                }),
                "{range} must rejoin the unbounded window path at length 10"
            );
        }
    }

    #[test]
    fn resolve_crossing_is_empty() {
        assert_eq!(deferred("2:-5").resolve(7), Some(SlicePlan::Empty));
        assert_eq!(deferred("2:-5").resolve(6), Some(SlicePlan::Empty));
        assert_eq!(deferred("-5:3").resolve(10), Some(SlicePlan::Empty));
    }

    #[test]
    fn from_end_plus() {
        // -5:+3 == s[-5:-2]
        assert_eq!(
            SliceRange::from_str("-5:+3").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(5).unwrap()),
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(2).unwrap())),
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn from_end_plus_exact_unbounds() {
        // -5:+5 reaches the end of input exactly == s[-5:]
        assert_eq!(SliceRange::from_str("-5:+5").unwrap().end, None);
    }

    #[test]
    fn from_end_plus_overflow_unbounds() {
        assert_eq!(SliceRange::from_str("-5:+7").unwrap().end, None);
    }

    #[test]
    fn from_end_plus_minus() {
        // -5:+-2 == s[-7:-3]
        assert_eq!(
            SliceRange::from_str("-5:+-2").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(7).unwrap()),
                end: Some(SliceIndex::FromEnd(NonZeroUsize::new(3).unwrap())),
                step: Step::forward(1),
            }
        );
        // -2:+-5 saturates past the end == s[-7:]
        assert_eq!(
            SliceRange::from_str("-2:+-5").unwrap(),
            SliceRange {
                start: SliceIndex::FromEnd(NonZeroUsize::new(7).unwrap()),
                end: None,
                step: Step::forward(1),
            }
        );
    }

    #[test]
    fn from_end_plus_zero_is_empty() {
        let range = SliceRange::from_str("-5:+0").unwrap();
        assert_eq!(
            range.end,
            Some(SliceIndex::FromEnd(NonZeroUsize::new(5).unwrap()))
        );
        assert_eq!(range.plan(), Plan::Resolved(SlicePlan::Empty));
    }

    mod explain {
        use super::*;

        // explain_tail re-derives plan()'s length-independent empty pairs to
        // pick the mirror-of-empty form; sweep every tail-relative shape so
        // the two derivations cannot drift apart silently.
        #[test]
        fn static_empty_rendering_agrees_with_plan() {
            let nz = |n| NonZeroUsize::new(n).unwrap();
            let mut ends = vec![None];
            ends.extend((0..=4).map(|end| Some(SliceIndex::FromStart(end))));
            ends.extend((1..=4).map(|m| Some(SliceIndex::FromEnd(nz(m)))));
            for k in 1..=4 {
                for &end in &ends {
                    let range = SliceRange {
                        start: SliceIndex::FromEnd(nz(k)),
                        end,
                        step: Step::forward(1),
                    };
                    let statically_empty = matches!(range.plan(), Plan::Resolved(SlicePlan::Empty));
                    assert_eq!(
                        range.explain("line").contains("count: 0"),
                        statically_empty,
                        "k={k} end={end:?}"
                    );
                }
            }
        }

        #[test]
        fn bounded_range_shows_count_and_positions() {
            let text = SliceRange::from_str("10:20").unwrap().explain("line");
            assert!(text.contains("start: 10"));
            assert!(text.contains("end:   20 (exclusive)"));
            assert!(text.contains("step:  1"));
            assert!(text.contains("0-based: lines at indices [10, 20)"));
            assert!(text.contains("from the 11th line to the 20th line"));
            assert!(text.contains("count: 10"));
        }

        #[test]
        fn unbounded_end_is_named() {
            let text = SliceRange::from_str(":").unwrap().explain("line");
            assert!(text.contains("end:   end of input"));
            assert!(text.contains("0-based: lines at indices [0, end of input)"));
            assert!(text.contains("to the last line"));
            assert!(text.contains("count: until end of input"));
        }

        #[test]
        fn extended_plus_minus_window_is_resolved() {
            let text = SliceRange::from_str("100:+-10").unwrap().explain("line");
            assert!(text.contains("start: 90"));
            assert!(text.contains("end:   110 (exclusive)"));
            assert!(text.contains("count: 20"));
        }

        #[test]
        fn extended_plus_window_is_resolved() {
            let text = SliceRange::from_str("5:+10").unwrap().explain("line");
            assert!(text.contains("start: 5"));
            assert!(text.contains("end:   15 (exclusive)"));
            assert!(text.contains("count: 10"));
        }

        #[test]
        fn step_count_rounds_up() {
            // indices 10,13,16,19 -> 4 elements
            let text = SliceRange::from_str("10:20:3").unwrap().explain("line");
            assert!(text.contains("step:  3"));
            assert!(text.contains("every 3 starting at 10"));
            assert!(text.contains("count: 4"));
        }

        #[test]
        fn empty_range_reports_zero() {
            let text = SliceRange::from_str("5:5").unwrap().explain("line");
            assert!(text.contains("empty"));
            assert!(text.contains("count: 0"));
        }

        #[test]
        fn max_start_saturates_first_position() {
            let range = SliceRange {
                start: SliceIndex::FromStart(usize::MAX),
                end: None,
                step: Step::forward(1),
            };
            let text = range.explain("line");
            assert!(text.contains("from the 18446744073709551615th line"));
        }

        #[test]
        fn unit_name_is_used() {
            let text = SliceRange::from_str("0:3").unwrap().explain("byte");
            assert!(text.contains("0-based: bytes at indices [0, 3)"));
            assert!(text.contains("from the 1st byte to the 3rd byte"));
        }

        #[test]
        fn negative_end_count_depends_on_length() {
            let text = SliceRange::from_str("2:-2").unwrap().explain("line");
            assert!(text.contains("start: 2"));
            assert!(text.contains("end:   2 from the end (exclusive)"));
            assert!(text
                .contains("0-based: lines at indices [2, length-2), clamped to the input length"));
            assert!(text.contains("1-based: from the 3rd line to the 3rd line from the end"));
            assert!(text.contains("count: depends on the input length"));

            let stepped = SliceRange::from_str("1:-1:2").unwrap().explain("line");
            assert!(stepped.contains("0-based: lines at indices [1, length-1), every 2 starting at 1, clamped to the input length"));
            assert!(stepped.contains(
                "1-based: every 2nd line from the 2nd line up to the 2nd line from the end"
            ));
            assert!(stepped.contains("count: depends on the input length"));
        }

        #[test]
        fn negative_start_is_symbolic() {
            let text = SliceRange::from_str("-5:").unwrap().explain("line");
            assert!(text.contains("start: 5 from the end"));
            assert!(text.contains("end:   end of input"));
            assert!(text.contains(
                "0-based: lines at indices [length-5, end of input), clamped to the input length"
            ));
            assert!(text.contains("1-based: from the 5th line from the end to the last line"));
            assert!(text.contains("count: at most 5"));

            let stepped = SliceRange::from_str("-7:-2:3").unwrap().explain("line");
            assert!(stepped.contains("start: 7 from the end"));
            assert!(stepped.contains("end:   2 from the end (exclusive)"));
            assert!(stepped.contains(
                "0-based: lines at indices [length-7, length-2), every 3 starting at length-7, clamped to the input length"
            ));
            assert!(stepped.contains(
                "1-based: every 3rd line from the 7th line from the end up to the 3rd line from the end"
            ));
            // indices L-7, L-4 -> at most 2 elements
            assert!(stepped.contains("count: at most 2"));

            let unbounded_stepped = SliceRange::from_str("-4::2").unwrap().explain("line");
            assert!(unbounded_stepped.contains(
                "0-based: lines at indices [length-4, end of input), every 2 starting at length-4, clamped to the input length"
            ));
            // indices L-4, L-2 -> at most 2 elements
            assert!(unbounded_stepped.contains("count: at most 2"));
        }

        #[test]
        fn negative_start_bounded_end_depends_on_length() {
            let text = SliceRange::from_str("-3:4").unwrap().explain("line");
            assert!(text.contains("start: 3 from the end"));
            assert!(text.contains("end:   4 (exclusive)"));
            assert!(text
                .contains("0-based: lines at indices [length-3, 4), clamped to the input length"));
            assert!(text.contains("1-based: from the 3rd line from the end to the 4th line"));
            assert!(text.contains("count: depends on the input length"));
        }

        #[test]
        fn negative_pair_empty_reports_zero() {
            let text = SliceRange::from_str("-2:-5").unwrap().explain("line");
            assert!(text.contains("0-based: lines at indices [length-2, length-5)"));
            assert!(text.contains("1-based: empty (start length-2 is at or past end length-5)"));
            assert!(text.contains("count: 0"));

            let text = SliceRange::from_str("-5:0").unwrap().explain("line");
            assert!(text.contains("1-based: empty (start length-5 is at or past end 0)"));
            assert!(text.contains("count: 0"));
        }
    }

    mod translate {
        use super::*;
        use TranslateDialect::{All, Awk, Bsd, Gnu, Posix};
        use TranslateMode::{Bytes, Chars, Custom, Lines};

        fn tr(range: &str, mode: TranslateMode, dialect: TranslateDialect) -> String {
            SliceRange::from_str(range)
                .unwrap()
                .translate(mode, dialect)
        }

        #[test]
        fn whole_input_is_cat() {
            assert_eq!(tr(":", Lines, Posix), "# posix\ncat\n");
            assert_eq!(tr("0::1", Bytes, Posix), "# posix\ncat\n");
        }

        // Chars mirror Custom: Copy stays cat (the whole input passes through
        // in every mode); every selecting or empty form has no equivalent.
        #[test]
        fn chars_have_no_single_command_equivalent() {
            assert_eq!(tr(":", Chars, Posix), "# posix\ncat\n");
            for range in ["1:5", "5:", "-3:", ":-2", "::2", "::-1", "5:3"] {
                assert_eq!(
                    tr(range, Chars, Gnu),
                    "# no equivalent: cut -c is per-line (and selects bytes on GNU), \
                     not a stream character slice\n",
                    "range {range}"
                );
            }
        }

        // Graphemes mirror Chars: cat for Copy, no equivalent otherwise.
        #[test]
        fn graphemes_have_no_single_command_equivalent() {
            use TranslateMode::Graphemes;
            assert_eq!(tr(":", Graphemes, Posix), "# posix\ncat\n");
            for range in ["1:5", "5:", "-3:", ":-2", "::2", "::-1", "5:3"] {
                assert_eq!(
                    tr(range, Graphemes, Gnu),
                    "# no equivalent: no standard tool selects by grapheme cluster\n",
                    "range {range}"
                );
            }
        }

        #[test]
        fn empty_range_gnu_zero_count_by_mode() {
            // Only GNU `head` accepts a zero count; POSIX/BSD reject it, so the
            // empty range is gnu-only, like the drop-last forms. Lines use
            // `head -n 0`, bytes `head -c 0`.
            assert_eq!(tr("5:3", Lines, Gnu), "# gnu\nhead -n 0\n");
            assert_eq!(
                tr("5:3", Lines, Posix),
                "# no equivalent: no POSIX single command selects an empty range; GNU has `head -n 0`\n"
            );
            assert!(tr("5:3", Lines, Bsd).starts_with("# no equivalent:"));
            assert!(tr("5:3", Lines, Awk).starts_with("# no equivalent:"));
            assert_eq!(tr("5:3", Bytes, Gnu), "# gnu\nhead -c 0\n");
            assert_eq!(
                tr("5:3", Bytes, Posix),
                "# no equivalent: no POSIX single command selects an empty range; GNU has `head -c 0`\n"
            );
            assert!(tr("5:3", Bytes, Bsd).starts_with("# no equivalent:"));
            assert!(tr("5:3", Bytes, Awk).starts_with("# no equivalent:"));
            // A custom delimiter has no equivalent even when empty, like every
            // other custom range — the zero-count head forms do not apply.
            assert_eq!(
                tr("5:3", Custom, Gnu),
                "# no equivalent: no standard tool selects records by a custom delimiter\n"
            );
            assert!(tr("5:3", Custom, Posix).starts_with("# no equivalent:"));
            // `:-0` normalizes to a FromStart(0) end, i.e. `:0`, which is empty.
            assert_eq!(tr(":-0", Lines, Gnu), "# gnu\nhead -n 0\n");
            assert!(tr(":-0", Lines, Posix).starts_with("# no equivalent:"));
        }

        #[test]
        fn head_and_tail_lines() {
            assert_eq!(tr(":5", Lines, Posix), "# posix\nhead -n 5\n");
            assert_eq!(tr("-5:", Lines, Posix), "# posix\ntail -n 5\n");
            assert_eq!(tr("9:", Lines, Posix), "# posix\ntail -n +10\n");
        }

        #[test]
        fn window_lines_use_sed() {
            assert_eq!(tr("1:5", Lines, Posix), "# posix\nsed -n '2,5p'\n");
            assert_eq!(tr("6:7", Lines, Posix), "# posix\nsed -n '7p'\n");
        }

        #[test]
        fn drop_last_lines_per_dialect() {
            // The last line has a POSIX single-command form; more do not.
            assert_eq!(tr(":-1", Lines, Posix), "# posix\nsed '$d'\n");
            assert_eq!(tr(":-1", Lines, Gnu), "# gnu\nhead -n -1\n");
            assert_eq!(tr(":-5", Lines, Gnu), "# gnu\nhead -n -5\n");
            assert_eq!(
                tr(":-5", Lines, Posix),
                "# no equivalent: no POSIX single command drops the last N lines; GNU has `head -n -N`\n"
            );
            assert!(tr(":-5", Lines, Bsd).starts_with("# no equivalent:"));
        }

        #[test]
        fn stepped_lines_posix_uses_awk_gnu_uses_sed() {
            assert_eq!(
                tr("::2", Lines, Posix),
                "# posix  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR%2==1'\n"
            );
            assert_eq!(tr("::2", Lines, Gnu), "# gnu\nsed -n '1~2p'\n");
            assert_eq!(tr("1::2", Lines, Gnu), "# gnu\nsed -n '2~2p'\n");
            assert_eq!(
                tr("3::2", Lines, Posix),
                "# posix  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR>=4 && (NR-4)%2==0'\n"
            );
            // Bounded stepped: every dialect uses awk (no clean bounded GNU sed).
            assert_eq!(
                tr("1:7:2", Lines, Gnu),
                "# gnu  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR>=2 && (NR-2)%2==0 && NR<=7'\n"
            );
        }

        #[test]
        fn awk_view_lines() {
            // Every awk form carries the divergence caveat inline, the unbounded one included.
            assert_eq!(
                tr(":5", Lines, Awk),
                "# awk  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR<=5'\n"
            );
            assert_eq!(
                tr("1:5", Lines, Awk),
                "# awk  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR>=2 && NR<=5'\n"
            );
            assert_eq!(
                tr("1:", Lines, Awk),
                "# awk  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR>=2'\n"
            );
            // awk cannot select relative to the end without buffering.
            assert!(tr("-5:", Lines, Awk).starts_with("# no equivalent:"));
            assert!(tr(":-1", Lines, Awk).starts_with("# no equivalent:"));
        }

        #[test]
        fn max_start_saturates_instead_of_overflowing() {
            // `start` can be usize::MAX; a plain `+ 1` would overflow-panic in
            // debug. Mirrors the explain test `max_start_saturates_first_position`.
            let max = usize::MAX.to_string();
            assert_eq!(
                tr(&format!("{max}:"), Lines, Posix),
                format!("# posix\ntail -n +{max}\n")
            );
            assert_eq!(
                tr(&format!("{max}:"), Bytes, Posix),
                format!("# posix\ntail -c +{max}\n")
            );
        }

        #[test]
        fn byte_head_dialects() {
            assert_eq!(
                tr(":5", Bytes, Posix),
                "# posix\ndd bs=1 count=5 2>/dev/null\n"
            );
            assert_eq!(
                tr(":5", Bytes, Bsd),
                "# bsd  (head -c is not in POSIX; present on BSD and GNU)\nhead -c 5\n"
            );
            assert_eq!(
                tr(":5", Bytes, Gnu),
                "# gnu  (head -c is not in POSIX; present on BSD and GNU)\nhead -c 5\n"
            );
        }

        #[test]
        fn byte_window_and_tail() {
            assert_eq!(
                tr("5:15", Bytes, Posix),
                "# posix\ndd bs=1 skip=5 count=10 2>/dev/null\n"
            );
            assert_eq!(tr("-5:", Bytes, Posix), "# posix\ntail -c 5\n");
            assert_eq!(tr("5:", Bytes, Posix), "# posix\ntail -c +6\n");
        }

        #[test]
        fn byte_drop_last_only_gnu() {
            assert_eq!(tr(":-5", Bytes, Gnu), "# gnu\nhead -c -5\n");
            assert_eq!(tr(":-1", Bytes, Gnu), "# gnu\nhead -c -1\n");
            assert!(tr(":-5", Bytes, Posix).starts_with("# no equivalent:"));
        }

        #[test]
        fn degenerate_stepped_byte_collapses_to_one_byte_window() {
            // A bounded byte step that never reaches a second selected byte picks
            // the single byte at `start`, identical to the unstepped one-byte
            // window, so it shares the dd/head -c translation.
            assert_eq!(
                tr("5:6:2", Bytes, Posix),
                "# posix\ndd bs=1 skip=5 count=1 2>/dev/null\n"
            );
            // end - start == step still selects only `start` (byte 5 for 5:7:2).
            assert_eq!(
                tr("5:7:2", Bytes, Posix),
                "# posix\ndd bs=1 skip=5 count=1 2>/dev/null\n"
            );
            // start == 0 reuses the head -c / dd-from-start spellings.
            assert_eq!(
                tr(":1:2", Bytes, Posix),
                "# posix\ndd bs=1 count=1 2>/dev/null\n"
            );
            assert_eq!(
                tr(":1:2", Bytes, Bsd),
                "# bsd  (head -c is not in POSIX; present on BSD and GNU)\nhead -c 1\n"
            );
            // A step that does reach a second byte stays a genuine, untranslatable
            // stride.
            assert!(tr("5:8:2", Bytes, Posix).starts_with("# no equivalent:"));
            assert!(tr(":3:2", Bytes, Posix).starts_with("# no equivalent:"));
        }

        #[test]
        fn untranslatable_cases() {
            // custom delimiter / NUL records
            assert!(tr(":5", Custom, Posix).starts_with("# no equivalent:"));
            // head-relative start, tail-relative end: needs length
            assert!(tr("3:-2", Lines, Posix).starts_with("# no equivalent:"));
            // tail-relative start with a fixed end: needs length
            assert!(tr("-5:3", Lines, Posix).starts_with("# no equivalent:"));
            // tail start combined with a step
            assert!(tr("-5::2", Lines, Posix).starts_with("# no equivalent:"));
            // strided bytes
            assert!(tr("::2", Bytes, Posix).starts_with("# no equivalent:"));
            // awk has no byte equivalent
            assert!(tr("5:15", Bytes, Awk).starts_with("# no equivalent:"));
        }

        #[test]
        fn relative_end_collapses_to_window() {
            assert_eq!(tr("5:+10", Lines, Posix), "# posix\nsed -n '6,15p'\n");
            assert_eq!(tr("100:+-10", Lines, Posix), "# posix\nsed -n '91,110p'\n");
        }

        #[test]
        fn all_lists_every_dialect_including_duplicates() {
            assert_eq!(
                tr("::2", Lines, All),
                "# posix  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR%2==1'\n# bsd  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR%2==1'\n# gnu\nsed -n '1~2p'\n# awk  (awk re-terminates an unterminated final line and may drop bytes after an embedded NUL)\nawk 'NR%2==1'\n"
            );
            // partial: only gnu has an equivalent
            assert_eq!(
                tr(":-5", Lines, All),
                "# posix  (no equivalent)\n# bsd  (no equivalent)\n# gnu\nhead -n -5\n# awk  (no equivalent)\n"
            );
        }

        #[test]
        fn all_lists_copy_and_empty_per_dialect() {
            let cat_block = "# posix\ncat\n# bsd\ncat\n# gnu\ncat\n# awk\ncat\n";
            assert_eq!(tr(":", Lines, All), cat_block);
            assert_eq!(tr("0::1", Bytes, All), cat_block);
            // The empty range is gnu-only (`head -n 0` for lines, `head -c 0`
            // for bytes); the rest report no equivalent, like the drop-last forms.
            // A custom delimiter has no equivalent in any dialect, even when empty.
            assert_eq!(
                tr("5:3", Lines, All),
                "# posix  (no equivalent)\n# bsd  (no equivalent)\n# gnu\nhead -n 0\n# awk  (no equivalent)\n"
            );
            assert_eq!(
                tr("5:3", Bytes, All),
                "# posix  (no equivalent)\n# bsd  (no equivalent)\n# gnu\nhead -c 0\n# awk  (no equivalent)\n"
            );
            assert_eq!(
                tr("5:3", Custom, All),
                "# posix  (no equivalent)\n# bsd  (no equivalent)\n# gnu  (no equivalent)\n# awk  (no equivalent)\n"
            );
        }

        #[test]
        fn every_output_ends_with_newline() {
            for (range, mode, dialect) in [
                (":", Lines, Posix),
                (":5", Bytes, Bsd),
                ("::2", Lines, All),
                (":-5", Lines, Posix),
                ("5:15", Bytes, Awk),
                ("5:3", Lines, Gnu),
            ] {
                assert!(
                    tr(range, mode, dialect).ends_with('\n'),
                    "{range} {mode:?} {dialect:?}"
                );
            }
        }
    }

    mod invalid {
        use super::*;

        #[test]
        fn empty() {
            assert!(SliceRange::from_str("").is_err());
        }

        #[test]
        fn non_integer_start() {
            assert!(SliceRange::from_str("a:1").is_err());
            assert!(SliceRange::from_str("a:1:1").is_err());
        }

        #[test]
        fn non_integer_end() {
            assert!(SliceRange::from_str("1:a").is_err());
            assert!(SliceRange::from_str("1:a:1").is_err());
        }

        #[test]
        fn non_integer_step() {
            assert!(SliceRange::from_str("1:1:b").is_err());
        }

        #[test]
        fn missing_colon() {
            let err = SliceRange::from_str("3").expect_err("bare number must be rejected");
            assert_eq!(err, ParseSliceRangeError::MissingColon);
            assert_eq!(
                err.to_string(),
                "range requires a ':' separator (e.g. '3:4', '3:', or ':3')"
            );
        }

        #[test]
        fn too_many_parts() {
            assert!(SliceRange::from_str("1:2:3:4").is_err());
            assert!(SliceRange::from_str("1:2:3:4:5").is_err());
            assert!(SliceRange::from_str(":::").is_err());
        }

        #[test]
        fn invalid_field_names_which_part() {
            assert!(matches!(
                SliceRange::from_str("a:1").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Start,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("1:a").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("1:1:b").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Step,
                    ..
                }
            ));
            assert!(SliceRange::from_str("1:5:x")
                .unwrap_err()
                .to_string()
                .starts_with("invalid step value 'x':"));
        }

        #[test]
        fn relative_end_requires_a_count() {
            assert_eq!(
                SliceRange::from_str("5:+").unwrap_err(),
                ParseSliceRangeError::MissingRelativeAmount
            );
            assert_eq!(
                SliceRange::from_str("5:+-").unwrap_err(),
                ParseSliceRangeError::MissingRelativeAmount
            );
        }

        #[test]
        fn relative_end_rejects_non_numeric_count() {
            assert!(matches!(
                SliceRange::from_str("1:+x").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
        }

        #[test]
        fn bare_minus_rejected() {
            let err = SliceRange::from_str(":-").unwrap_err();
            assert!(matches!(
                err,
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(err.to_string().starts_with("invalid end value '-':"));

            let err = SliceRange::from_str("-:").unwrap_err();
            assert!(matches!(
                err,
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Start,
                    ..
                }
            ));
            assert!(err.to_string().starts_with("invalid start value '-':"));
        }

        #[test]
        fn double_minus_rejected() {
            assert!(matches!(
                SliceRange::from_str(":--1").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("--1:").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Start,
                    ..
                }
            ));
        }

        #[test]
        fn minus_plus_rejected() {
            assert!(matches!(
                SliceRange::from_str(":-+1").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("-+1:").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Start,
                    ..
                }
            ));
        }

        #[test]
        fn negative_overflow_rejected() {
            assert!(matches!(
                SliceRange::from_str(":-18446744073709551616").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::End,
                    ..
                }
            ));
            assert!(matches!(
                SliceRange::from_str("-18446744073709551616:").unwrap_err(),
                ParseSliceRangeError::InvalidField {
                    field: RangeField::Start,
                    ..
                }
            ));
        }

        #[test]
        fn zero_and_malformed_steps_stay_rejected() {
            for step in ["::0", "::-", "::-0", "::-+1", "::--1"] {
                assert!(
                    matches!(
                        SliceRange::from_str(step).unwrap_err(),
                        ParseSliceRangeError::InvalidField {
                            field: RangeField::Step,
                            ..
                        }
                    ),
                    "{step} must keep rejecting its step"
                );
            }
        }

        #[test]
        fn digit_shaped_rejections_report_their_own_cause() {
            // The magnitude's own failure, not the sign parse's
            // "invalid digit" tripped by the '-'.
            for (range, cause) in [
                ("::-99999999999999999999", "number too large"),
                ("::-0", "zero"),
                ("-99999999999999999999:", "number too large"),
            ] {
                let err = SliceRange::from_str(range).unwrap_err();
                assert!(
                    err.to_string().contains(cause),
                    "{range} must report its cause, got: {err}"
                );
            }
        }

        #[test]
        fn negative_step_parses_reverse() {
            assert_eq!(
                SliceRange::from_str("::-1").unwrap(),
                SliceRange {
                    start: SliceIndex::FromEnd(NonZeroUsize::MIN),
                    end: None,
                    step: Step::Backward(NonZeroUsize::MIN),
                }
            );
            // An explicit start keeps its parsed form instead of the
            // last-element default.
            assert_eq!(
                SliceRange::from_str("5:1:-2").unwrap(),
                SliceRange {
                    start: SliceIndex::FromStart(5),
                    end: Some(SliceIndex::FromStart(1)),
                    step: Step::Backward(NonZeroUsize::new(2).unwrap()),
                }
            );
        }

        #[test]
        fn negative_step_rejects_relative_end() {
            for range in ["5:+3:-1", "5:+-3:-1"] {
                assert!(
                    matches!(
                        SliceRange::from_str(range).unwrap_err(),
                        ParseSliceRangeError::NegativeStepWithRelativeEnd
                    ),
                    "{range} must reject a relative end with a reverse step"
                );
            }
        }
    }
}
