// Failure modes covered: empty text, spans outside the line, reversed spans,
// spans that start or end inside a multi-byte character, overlapping spans,
// adjacent equal spans, and the marked/error overlays splitting a span.
use super::*;
use HighlightKind::*;

fn seg(len: usize, kind: Option<HighlightKind>, marked: bool, error: bool) -> Segment {
    Segment {
        len,
        kind,
        marked,
        error,
    }
}

#[test]
fn empty_text_has_no_segments() {
    assert!(segments("", &[(0..3, Keyword)], None, None).is_empty());
}

#[test]
fn unstyled_text_is_one_segment() {
    assert_eq!(
        segments("hello", &[], None, None),
        vec![seg(5, None, false, false)]
    );
}

#[test]
fn spans_split_the_line() {
    assert_eq!(
        segments("int x", &[(0..3, Type)], None, None),
        vec![seg(3, Some(Type), false, false), seg(2, None, false, false)]
    );
}

#[test]
fn adjacent_equal_segments_merge() {
    assert_eq!(
        segments("ab", &[(0..1, Keyword), (1..2, Keyword)], None, None),
        vec![seg(2, Some(Keyword), false, false)]
    );
}

#[test]
fn marked_and_error_overlays_split_a_span() {
    assert_eq!(
        segments("abcdef", &[(0..6, String)], Some(1..3), Some(2..5)),
        vec![
            seg(1, Some(String), false, false),
            seg(1, Some(String), true, false),
            seg(1, Some(String), true, true),
            seg(2, Some(String), false, true),
            seg(1, Some(String), false, false),
        ]
    );
}

#[test]
fn bad_spans_are_clamped_or_ignored() {
    assert_eq!(
        segments("abc", &[(0..99, Number)], None, None),
        vec![seg(3, Some(Number), false, false)]
    );
    assert_eq!(
        segments("abc", &[(7..9, Number)], None, None),
        vec![seg(3, None, false, false)]
    );
    #[allow(clippy::reversed_empty_ranges)]
    let reversed = [(3..1, Number)];
    assert_eq!(
        segments("abc", &reversed, None, None),
        vec![seg(3, None, false, false)]
    );
}

#[test]
fn overlapping_spans_resolve_to_the_first() {
    assert_eq!(
        segments("abcdef", &[(0..4, String), (2..6, Comment)], None, None),
        vec![
            seg(4, Some(String), false, false),
            seg(2, Some(Comment), false, false)
        ]
    );
}

#[test]
fn spans_inside_a_multibyte_char_snap_outward() {
    let out = segments("日本", &[(1..4, Keyword)], None, None);
    assert_eq!(out, vec![seg(6, Some(Keyword), false, false)]);
}

#[test]
fn invariants_hold_for_every_small_span_combination() {
    let text = "a日b😀";
    let len = text.len();
    let mut ranges = Vec::new();
    for s in 0..=len + 2 {
        for e in 0..=len + 2 {
            ranges.push(s..e);
        }
    }
    for a in &ranges {
        for b in ranges.iter().step_by(3) {
            let out = segments(
                text,
                &[(a.clone(), Keyword), (b.clone(), Comment)],
                Some(b.clone()),
                Some(a.clone()),
            );
            let mut at = 0;
            for s in &out {
                assert!(s.len > 0);
                at += s.len;
                assert!(text.is_char_boundary(at), "{a:?} {b:?} split at {at}");
            }
            assert_eq!(at, len, "{a:?} {b:?}");
        }
    }
}
