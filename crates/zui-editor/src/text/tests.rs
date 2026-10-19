// Failure modes covered: multi-byte UTF-8 boundaries (2/3/4-byte chars),
// combining marks (one grapheme, several chars), CRLF and lone CR, tabs that
// expand to a variable number of spaces, a display offset that lands inside a
// tab, columns past the end of a line, and empty lines.
use super::*;

#[test]
fn newlines_are_normalised() {
    assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
    assert_eq!(normalize_newlines("a\r\n\r\nb"), "a\n\nb");
    assert_eq!(normalize_newlines("\r"), "\n");
}

#[test]
fn word_left_skips_space_then_one_class_run() {
    assert_eq!(prev_word_boundary("foo bar", 7), 4);
    assert_eq!(prev_word_boundary("foo bar", 4), 0);
    assert_eq!(prev_word_boundary("foo   bar", 6), 0);
    assert_eq!(prev_word_boundary("a.b", 3), 2);
    assert_eq!(prev_word_boundary("a.b", 2), 1);
    assert_eq!(prev_word_boundary("foo", 0), 0);
    assert_eq!(prev_word_boundary("日本 x", 8), 7);
    assert_eq!(prev_word_boundary("日本 x", 7), 0);
}

#[test]
fn word_right_mirrors_word_left() {
    assert_eq!(next_word_boundary("foo bar", 0), 3);
    assert_eq!(next_word_boundary("foo bar", 3), 7);
    assert_eq!(next_word_boundary("a.b", 0), 1);
    assert_eq!(next_word_boundary("a.b", 1), 2);
    assert_eq!(next_word_boundary("foo", 3), 3);
    assert_eq!(next_word_boundary("日本 x", 0), 6);
    assert_eq!(next_word_boundary("under_score x", 0), 11);
}

#[test]
fn word_range_selects_the_run_under_the_column() {
    assert_eq!(word_range_at("foo  bar", 1), 0..3);
    assert_eq!(word_range_at("foo  bar", 4), 3..5);
    assert_eq!(word_range_at("foo", 3), 0..3);
    assert_eq!(word_range_at("", 0), 0..0);
    assert_eq!(word_range_at("日本 x", 3), 0..6);
    assert_eq!(word_range_at("a->b", 2), 1..3);
}

#[test]
fn first_nonblank_counts_bytes_of_leading_whitespace() {
    assert_eq!(first_nonblank("  \tx"), 3);
    assert_eq!(first_nonblank("x"), 0);
    assert_eq!(first_nonblank("   "), 3);
    assert_eq!(first_nonblank(""), 0);
}

#[test]
fn graphemes_move_over_whole_clusters() {
    let s = "ae\u{301}b";
    assert_eq!(next_grapheme(s, 1), 4);
    assert_eq!(prev_grapheme(s, 4), 1);
    assert_eq!(next_grapheme("日本", 0), 3);
    assert_eq!(prev_grapheme("日本", 6), 3);
    assert_eq!(prev_grapheme("😀", 4), 0);
    assert_eq!(next_grapheme("a", 1), 1);
    assert_eq!(prev_grapheme("a", 0), 0);
}

#[test]
fn tabs_expand_to_the_next_stop() {
    assert_eq!(expand_tabs("a\tb", 4), "a   b");
    assert_eq!(expand_tabs("\t\tx", 4), "        x");
    assert_eq!(expand_tabs("abcd\te", 4), "abcd    e");
    assert!(matches!(expand_tabs("plain", 4), Cow::Borrowed(_)));
}

#[test]
fn display_offsets_round_trip_around_tabs() {
    let line = "a\tb";
    assert_eq!(to_display(line, 0, 4), 0);
    assert_eq!(to_display(line, 1, 4), 1);
    assert_eq!(to_display(line, 2, 4), 4);
    assert_eq!(to_display(line, 3, 4), 5);
    assert_eq!(from_display(line, 4, 4), 2);
    assert_eq!(from_display(line, 5, 4), 3);
    // Inside the tab: nearer edge wins.
    assert_eq!(from_display(line, 2, 4), 1);
    assert_eq!(from_display(line, 3, 4), 2);
    // Past the end clamps to the line length.
    assert_eq!(from_display(line, 99, 4), 3);
}

#[test]
fn display_offsets_are_identity_without_tabs_even_for_multibyte() {
    let line = "日a";
    assert_eq!(to_display(line, 3, 4), 3);
    assert_eq!(from_display(line, 3, 4), 3);
}

#[test]
fn display_columns_count_chars_not_bytes() {
    assert_eq!(display_col("日本x", 6, 4), 2);
    assert_eq!(display_col("\tx", 1, 4), 4);
    assert_eq!(display_col("ab\tx", 3, 4), 4);
    assert_eq!(byte_at_col("日本x", 2, 4), 6);
    assert_eq!(byte_at_col("日本x", 99, 4), 7);
    assert_eq!(byte_at_col("\tx", 1, 4), 0); // inside the tab: nearer boundary
    assert_eq!(byte_at_col("\tx", 3, 4), 1);
    assert_eq!(byte_at_col("\tx", 4, 4), 1);
    assert_eq!(byte_at_col("", 3, 4), 0);
}

#[test]
fn utf16_length_counts_surrogate_pairs() {
    assert_eq!(utf16_len("a😀"), 3);
    assert_eq!(utf16_len("日"), 1);
    assert_eq!(utf16_len(""), 0);
}
