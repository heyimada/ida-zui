// Merges highlight spans with the IME marked range and an error underline into
// non-overlapping segments for one line. No gpui types.
use crate::highlight::HighlightKind;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub len: usize,
    pub kind: Option<HighlightKind>,
    pub marked: bool,
    pub error: bool,
}

/// Where spans overlap the first one wins. Ranges are clamped to the line and
/// snapped outward to char boundaries; empty or reversed ranges are ignored.
pub fn segments(
    text: &str,
    spans: &[(Range<usize>, HighlightKind)],
    marked: Option<Range<usize>>,
    error: Option<Range<usize>>,
) -> Vec<Segment> {
    let sanitize = |r: &Range<usize>| -> Option<Range<usize>> {
        let mut start = r.start.min(text.len());
        let mut end = r.end.min(text.len());
        while !text.is_char_boundary(start) {
            start -= 1;
        }
        while !text.is_char_boundary(end) {
            end += 1;
        }
        (start < end).then_some(start..end)
    };
    let mut kinds: Vec<Option<HighlightKind>> = vec![None; text.len()];
    for (range, kind) in spans {
        if let Some(r) = sanitize(range) {
            for slot in &mut kinds[r] {
                slot.get_or_insert(*kind);
            }
        }
    }
    let marked = marked.as_ref().and_then(sanitize);
    let error = error.as_ref().and_then(sanitize);
    let inside = |r: &Option<Range<usize>>, i: usize| r.as_ref().is_some_and(|r| r.contains(&i));

    let mut out: Vec<Segment> = Vec::new();
    for (i, c) in text.char_indices() {
        let seg = Segment {
            len: c.len_utf8(),
            kind: kinds[i],
            marked: inside(&marked, i),
            error: inside(&error, i),
        };
        match out.last_mut() {
            Some(last)
                if (last.kind, last.marked, last.error) == (seg.kind, seg.marked, seg.error) =>
            {
                last.len += seg.len
            }
            _ => out.push(seg),
        }
    }
    out
}

#[cfg(test)]
mod tests;
