// Failure modes covered: empty buffer, last line without newline, trailing
// newline, CRLF/CR in set_text and paste, stray control characters,
// multi-byte characters under backspace/arrows/vertical moves, goal column
// through short lines (chars and tabs), selection reversal, undo grouping
// (consecutive typing, cursor move, newline, selection replace, backspace
// runs), redo cleared by new edits, undo restoring the selection, indent and
// outdent with mixed tabs/blank lines/selection ending at column 0, auto
// indent (copy, extra after an opener, split of `{}`), joins at line
// boundaries, smart home toggling, UTF-16 conversion with surrogate pairs,
// out-of-range and mid-character selections.
use super::*;

/// `|` marks the caret, `«` the anchor and `»` the head of a selection.
fn mk(s: &str) -> Doc {
    let mut text = String::new();
    let (mut anchor, mut head) = (None, None);
    let (mut line, mut col) = (0, 0);
    for c in s.chars() {
        let here = Pos { line, col };
        match c {
            '|' => {
                anchor = Some(here);
                head = Some(here);
            }
            '«' => anchor = Some(here),
            '»' => head = Some(here),
            '\n' => {
                text.push(c);
                line += 1;
                col = 0;
            }
            _ => {
                text.push(c);
                col += c.len_utf8();
            }
        }
    }
    let mut d = Doc::new(&text);
    d.set_selection(anchor.unwrap_or_default(), head.unwrap_or_default());
    d
}

fn show(d: &Doc) -> String {
    let (a, h) = (d.anchor(), d.head());
    let mut out = String::new();
    for (i, l) in d.lines().iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut marks: Vec<(usize, char)> = Vec::new();
        if a == h {
            if a.line == i {
                marks.push((a.col, '|'));
            }
        } else {
            if a.line == i {
                marks.push((a.col, '«'));
            }
            if h.line == i {
                marks.push((h.col, '»'));
            }
        }
        marks.sort_by_key(|m| m.0);
        let mut last = 0;
        for (col, c) in marks {
            out.push_str(&l[last..col]);
            out.push(c);
            last = col;
        }
        out.push_str(&l[last..]);
    }
    out
}

fn type_str(d: &mut Doc, s: &str) {
    for c in s.chars() {
        d.insert(&c.to_string());
    }
}

#[test]
fn empty_buffer_is_one_empty_line_and_edits_are_noops() {
    let mut d = Doc::new("");
    assert_eq!((d.line_count(), d.text().as_str()), (1, ""));
    d.backspace();
    d.delete();
    d.delete_word_left();
    d.delete_word_right();
    d.outdent();
    d.move_left(false);
    d.move_right(false);
    d.move_up(false);
    d.move_down(true);
    d.move_word_left(false);
    d.move_word_right(false);
    d.move_end(false);
    d.move_home(false);
    assert!(!d.undo());
    assert!(!d.redo());
    assert_eq!(show(&d), "|");
}

#[test]
fn text_round_trips_with_and_without_trailing_newline() {
    assert_eq!(Doc::new("a\nb").text(), "a\nb");
    let d = Doc::new("a\n");
    assert_eq!((d.line_count(), d.text().as_str()), (2, "a\n"));
    assert_eq!(Doc::new("\n\n").line_count(), 3);
}

#[test]
fn crlf_and_cr_are_normalised_in_set_text_and_insert() {
    let d = Doc::new("a\r\nb\rc");
    assert_eq!(d.text(), "a\nb\nc");
    let mut d = mk("|");
    d.insert("x\r\ny\r\n");
    assert_eq!(d.text(), "x\ny\n");
    assert_eq!(show(&d), "x\ny\n|");
}

#[test]
fn control_characters_are_dropped_but_tabs_stay() {
    let mut d = mk("|");
    d.insert("a\u{7}b\tc\u{0}");
    assert_eq!(d.text(), "ab\tc");
}

#[test]
fn backspace_and_arrows_respect_multibyte_boundaries() {
    let mut d = mk("日本|");
    d.backspace();
    assert_eq!(show(&d), "日|");
    d.move_left(false);
    assert_eq!(show(&d), "|日");
    d.move_right(false);
    assert_eq!(show(&d), "日|");
    let mut d = mk("a|😀b");
    d.move_right(false);
    assert_eq!(show(&d), "a😀|b");
    d.backspace();
    assert_eq!(show(&d), "a|b");
}

#[test]
fn arrows_cross_line_boundaries() {
    let mut d = mk("ab\n|cd");
    d.move_left(false);
    assert_eq!(show(&d), "ab|\ncd");
    d.move_right(false);
    assert_eq!(show(&d), "ab\n|cd");
}

#[test]
fn vertical_moves_keep_the_goal_column_through_short_lines() {
    let mut d = mk("hello|\nhi\nworld");
    d.move_down(false);
    assert_eq!(show(&d), "hello\nhi|\nworld");
    d.move_down(false);
    assert_eq!(show(&d), "hello\nhi\nworld|");
    d.move_up(false);
    d.move_up(false);
    assert_eq!(show(&d), "hello|\nhi\nworld");
    // A horizontal move resets the goal.
    let mut d = mk("hello|\nhi\nworld");
    d.move_down(false);
    d.move_left(false);
    d.move_down(false);
    assert_eq!(show(&d), "hello\nhi\nw|orld");
}

#[test]
fn goal_column_is_measured_in_chars_and_tab_stops_not_bytes() {
    let mut d = mk("日本語|\nabcdef");
    d.move_down(false);
    assert_eq!(show(&d), "日本語\nabc|def");
    let mut d = mk("\tx|\nabcdefgh");
    d.move_down(false);
    assert_eq!(show(&d), "\tx\nabcde|fgh");
}

#[test]
fn up_on_first_line_goes_to_start_and_down_on_last_goes_to_end() {
    let mut d = mk("ab|c\nd");
    d.move_up(false);
    assert_eq!(show(&d), "|abc\nd");
    let mut d = mk("a\nb|c");
    d.move_down(false);
    assert_eq!(show(&d), "a\nbc|");
}

#[test]
fn selection_reverses_and_shrinks_around_a_fixed_anchor() {
    let mut d = mk("ab|cd");
    d.move_left(true);
    d.move_left(true);
    assert_eq!((d.anchor().col, d.head().col), (2, 0));
    assert_eq!(
        d.selection(),
        Pos { line: 0, col: 0 }..Pos { line: 0, col: 2 }
    );
    d.move_right(true);
    assert_eq!((d.anchor().col, d.head().col), (2, 1));
    d.move_right(true);
    d.move_right(true);
    assert_eq!((d.anchor().col, d.head().col), (2, 3));
    assert_eq!(d.selected_text(), "c");
}

#[test]
fn plain_arrows_collapse_a_selection_to_its_edge() {
    let mut d = mk("a«bc»d");
    d.move_left(false);
    assert_eq!(show(&d), "a|bcd");
    let mut d = mk("a«bc»d");
    d.move_right(false);
    assert_eq!(show(&d), "abc|d");
    let mut d = mk("a»bc«d");
    d.move_right(false);
    assert_eq!(show(&d), "abc|d");
}

#[test]
fn typing_over_a_selection_replaces_it() {
    let mut d = mk("a«bc»d");
    d.insert("X");
    assert_eq!(show(&d), "aX|d");
    d.select_all();
    d.insert("Z");
    assert_eq!(show(&d), "Z|");
}

#[test]
fn consecutive_typing_is_one_undo_step() {
    let mut d = mk("|");
    type_str(&mut d, "abc");
    assert!(d.undo());
    assert_eq!(show(&d), "|");
    assert!(d.redo());
    assert_eq!(show(&d), "abc|");
    assert!(!d.redo());
}

#[test]
fn a_cursor_move_between_keystrokes_splits_the_group() {
    let mut d = mk("|");
    d.insert("a");
    d.move_home(false);
    d.insert("b");
    assert_eq!(d.text(), "ba");
    d.undo();
    assert_eq!(d.text(), "a");
    d.undo();
    assert_eq!(d.text(), "");
}

#[test]
fn newline_and_selection_replacement_break_groups() {
    let mut d = mk("|");
    type_str(&mut d, "a");
    d.newline();
    type_str(&mut d, "bc");
    assert_eq!(d.text(), "a\nbc");
    d.undo();
    assert_eq!(d.text(), "a\n");
    d.undo();
    assert_eq!(d.text(), "a");
    d.undo();
    assert_eq!(d.text(), "");

    let mut d = mk("|");
    type_str(&mut d, "ab");
    d.select_all();
    d.insert("Z");
    d.undo();
    assert_eq!(show(&d), "«ab»");
}

#[test]
fn backspace_runs_group_but_not_with_typing() {
    let mut d = mk("abcd|");
    d.backspace();
    d.backspace();
    d.backspace();
    assert_eq!(d.text(), "a");
    d.undo();
    assert_eq!(show(&d), "abcd|");
    d.redo();
    type_str(&mut d, "xy");
    d.backspace();
    d.undo();
    assert_eq!(d.text(), "axy");
    d.undo();
    assert_eq!(d.text(), "a");
}

#[test]
fn a_new_edit_clears_redo() {
    let mut d = mk("|");
    type_str(&mut d, "a");
    d.undo();
    type_str(&mut d, "b");
    assert!(!d.redo());
    assert_eq!(d.text(), "b");
}

#[test]
fn undo_and_redo_restore_the_selection() {
    let mut d = mk("«abc»");
    d.insert("x");
    assert_eq!(show(&d), "x|");
    d.undo();
    assert_eq!(show(&d), "«abc»");
    d.redo();
    assert_eq!(show(&d), "x|");
    let mut d = mk("»abc«");
    d.insert("x");
    d.undo();
    assert_eq!(show(&d), "»abc«");
}

#[test]
fn undoing_a_multi_line_paste_is_one_step() {
    let mut d = mk("x|");
    d.insert("a\nb\nc");
    assert_eq!(show(&d), "xa\nb\nc|");
    d.undo();
    assert_eq!(show(&d), "x|");
}

#[test]
fn enter_copies_the_indentation_before_the_caret() {
    let mut d = mk("    foo|");
    d.newline();
    assert_eq!(show(&d), "    foo\n    |");
    let mut d = mk("  |  foo");
    d.newline();
    assert_eq!(show(&d), "  \n  |  foo");
    let mut d = mk("\tfoo|");
    d.newline();
    assert_eq!(show(&d), "\tfoo\n\t|");
}

#[test]
fn enter_after_an_opener_adds_a_level_and_splits_empty_braces() {
    let mut d = mk("struct S {|");
    d.newline();
    assert_eq!(show(&d), "struct S {\n    |");
    let mut d = mk("  if (x) {|}");
    d.newline();
    assert_eq!(show(&d), "  if (x) {\n      |\n  }");
    let mut d = mk("\tf {|");
    d.newline();
    assert_eq!(show(&d), "\tf {\n\t\t|");
    d.undo();
    assert_eq!(show(&d), "\tf {|");
}

#[test]
fn colon_opens_a_block_only_when_configured() {
    let mut d = mk("def f():|");
    d.newline();
    assert_eq!(show(&d), "def f():\n|");
    let mut d = mk("def f():|");
    d.set_indent_openers("{:");
    d.newline();
    assert_eq!(show(&d), "def f():\n    |");
}

#[test]
fn enter_replaces_the_selection() {
    let mut d = mk("a«bc»d");
    d.newline();
    assert_eq!(show(&d), "a\n|d");
}

#[test]
fn tab_without_a_multi_line_selection_inserts_to_the_next_stop() {
    let mut d = mk("a|");
    d.indent();
    assert_eq!(show(&d), "a   |");
    let mut d = mk("|");
    d.indent();
    assert_eq!(show(&d), "    |");
    let mut d = mk("a«bc»d");
    d.indent();
    assert_eq!(show(&d), "a   |d");
}

#[test]
fn indent_applies_to_every_selected_line_except_blank_ones() {
    let mut d = mk("«a\n\nb»");
    d.indent();
    assert_eq!(show(&d), "«    a\n\n    b»");
    d.undo();
    assert_eq!(show(&d), "«a\n\nb»");
}

#[test]
fn indent_skips_the_last_line_when_the_selection_ends_at_column_zero() {
    let mut d = mk("«a\nb\n»c");
    d.indent();
    assert_eq!(show(&d), "«    a\n    b\n»c");
}

#[test]
fn outdent_handles_spaces_tabs_and_mixed_indentation() {
    let mut d = mk("    a|");
    d.outdent();
    assert_eq!(show(&d), "a|");
    let mut d = mk("\tfoo|");
    d.outdent();
    assert_eq!(show(&d), "foo|");
    let mut d = mk("  \tfoo");
    d.outdent();
    assert_eq!(d.text(), "foo");
    let mut d = mk("      x|");
    d.outdent();
    assert_eq!(show(&d), "  x|");
    let mut d = mk("  | x");
    d.outdent();
    assert_eq!(show(&d), "|x");
    let mut d = mk("«  a\n\tb\nc»");
    d.outdent();
    assert_eq!(d.text(), "a\nb\nc");
}

#[test]
fn outdent_with_nothing_to_remove_makes_no_undo_step() {
    let mut d = mk("foo|");
    d.outdent();
    assert!(!d.undo());
}

#[test]
fn backspace_and_delete_join_lines() {
    let mut d = mk("ab\n|cd");
    d.backspace();
    assert_eq!(show(&d), "ab|cd");
    let mut d = mk("ab|\ncd");
    d.delete();
    assert_eq!(show(&d), "ab|cd");
    let mut d = mk("ab|");
    d.delete();
    assert_eq!(show(&d), "ab|");
}

#[test]
fn backspace_and_delete_remove_a_selection_first() {
    let mut d = mk("a«bc»d");
    d.backspace();
    assert_eq!(show(&d), "a|d");
    let mut d = mk("a«bc»d");
    d.delete();
    assert_eq!(show(&d), "a|d");
}

#[test]
fn word_deletion_crosses_spaces_and_line_starts() {
    let mut d = mk("foo bar|");
    d.delete_word_left();
    assert_eq!(show(&d), "foo |");
    let mut d = mk("foo  |bar");
    d.delete_word_left();
    assert_eq!(show(&d), "|bar");
    let mut d = mk("foo\n|bar");
    d.delete_word_left();
    assert_eq!(show(&d), "foo|bar");
    let mut d = mk("|foo bar");
    d.delete_word_right();
    assert_eq!(show(&d), "| bar");
    let mut d = mk("foo|\nbar");
    d.delete_word_right();
    assert_eq!(show(&d), "foo|bar");
}

#[test]
fn word_moves_cross_lines() {
    let mut d = mk("foo bar|");
    d.move_word_left(false);
    assert_eq!(show(&d), "foo |bar");
    d.move_word_left(false);
    d.move_word_left(false);
    assert_eq!(show(&d), "|foo bar");
    let mut d = mk("foo\n|bar");
    d.move_word_left(false);
    assert_eq!(show(&d), "foo|\nbar");
    let mut d = mk("foo|\nbar");
    d.move_word_right(false);
    assert_eq!(show(&d), "foo\n|bar");
    let mut d = mk("|a.b");
    d.move_word_right(false);
    d.move_word_right(false);
    assert_eq!(show(&d), "a.|b");
}

#[test]
fn smart_home_toggles_between_first_nonblank_and_column_zero() {
    let mut d = mk("    fo|o");
    d.move_home(false);
    assert_eq!(show(&d), "    |foo");
    d.move_home(false);
    assert_eq!(show(&d), "|    foo");
    d.move_home(false);
    assert_eq!(show(&d), "    |foo");
    let mut d = mk("    |");
    d.move_home(false);
    assert_eq!(show(&d), "|    ");
    let mut d = mk("    fo|o");
    d.move_home(true);
    assert_eq!(show(&d), "    »fo«o");
}

#[test]
fn end_and_document_extremes() {
    let mut d = mk("a|b\ncd\nef");
    d.move_end(false);
    assert_eq!(show(&d), "ab|\ncd\nef");
    d.move_doc_end(false);
    assert_eq!(show(&d), "ab\ncd\nef|");
    d.move_doc_start(true);
    assert_eq!(
        (d.anchor(), d.head()),
        (Pos { line: 2, col: 2 }, Pos::default())
    );
}

#[test]
fn page_moves_clamp_to_the_document() {
    let mut d = mk("a\nb\nc\nd|");
    d.move_lines(-10, false);
    assert_eq!(d.head().line, 0);
    d.move_lines(2, false);
    assert_eq!(d.head().line, 2);
    d.move_lines(99, false);
    assert_eq!(d.head().line, 3);
}

#[test]
fn double_click_word_and_line_selection() {
    let mut d = mk("foo bar");
    d.select_word_at(Pos { line: 0, col: 5 });
    assert_eq!(d.selected_text(), "bar");
    d.select_line(0);
    assert_eq!(d.selected_text(), "foo bar");
    let mut d = mk("a\nb\nc");
    d.select_line(1);
    assert_eq!(d.selected_text(), "b\n");
    d.select_line(2);
    assert_eq!(d.selected_text(), "c");
}

#[test]
fn duplicate_line_copies_below_and_moves_the_caret_down() {
    let mut d = mk("a|b\nc");
    d.duplicate_line();
    assert_eq!(show(&d), "ab\na|b\nc");
    let mut d = mk("x|");
    d.duplicate_line();
    assert_eq!(show(&d), "x\nx|");
    let mut d = mk("«a\nb»\nc");
    d.duplicate_line();
    assert_eq!(d.text(), "a\nb\na\nb\nc");
    d.undo();
    assert_eq!(d.text(), "a\nb\nc");
}

#[test]
fn utf16_offsets_count_surrogates_and_newlines() {
    let d = Doc::new("a😀\nb");
    assert_eq!(d.pos_to_utf16(Pos { line: 0, col: 5 }), 3);
    assert_eq!(d.pos_to_utf16(Pos { line: 1, col: 0 }), 4);
    assert_eq!(d.utf16_to_pos(4), Pos { line: 1, col: 0 });
    assert_eq!(d.utf16_to_pos(3), Pos { line: 0, col: 5 });
    assert_eq!(d.utf16_to_pos(2), Pos { line: 0, col: 1 }); // inside the pair: floor
    assert_eq!(d.utf16_to_pos(999), Pos { line: 1, col: 1 });
}

#[test]
fn set_selection_clamps_and_snaps_to_char_boundaries() {
    let mut d = Doc::new("日本");
    d.set_selection(Pos { line: 9, col: 9 }, Pos { line: 0, col: 1 });
    assert_eq!(d.anchor(), Pos { line: 0, col: 6 });
    assert_eq!(d.head(), Pos { line: 0, col: 0 });
}

#[test]
fn replace_range_across_lines_reports_line_edits() {
    let mut d = Doc::new("a\nb\nc");
    d.take_line_edits();
    d.replace_range(
        Pos { line: 0, col: 1 }..Pos { line: 2, col: 0 },
        "X\nY\nZ\n",
    );
    assert_eq!(d.text(), "aX\nY\nZ\nc");
    assert_eq!(
        d.take_line_edits(),
        vec![LineEdit {
            start_line: 0,
            removed: 3,
            inserted: 4
        }]
    );
    assert!(d.take_line_edits().is_empty());
}

#[test]
fn set_text_discards_history() {
    let mut d = mk("|");
    type_str(&mut d, "abc");
    d.set_text("fresh");
    assert!(!d.undo());
    assert_eq!(d.text(), "fresh");
}
