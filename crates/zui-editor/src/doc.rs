// The text model: lines, selection, edits and undo. No gpui types.
use crate::text::{self, TAB_WIDTH};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

/// A change to the line array: `removed` lines starting at `start_line` were
/// replaced by `inserted` lines. Consumers (the highlight cache) replay these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEdit {
    pub start_line: usize,
    pub removed: usize,
    pub inserted: usize,
}

const INDENT: &str = "    ";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Typing,
    Backspace,
    Delete,
    Other,
}

struct Rec {
    start: Pos,
    removed: String,
    inserted: String,
}

struct Group {
    recs: Vec<Rec>,
    kind: Kind,
    before: (Pos, Pos),
    after: (Pos, Pos),
}

pub struct Doc {
    lines: Vec<String>,
    anchor: Pos,
    head: Pos,
    /// Character column kept through consecutive vertical moves.
    goal: Option<usize>,
    undo: Vec<Group>,
    redo: Vec<Group>,
    /// Whether the newest undo group may absorb the next keystroke.
    open: bool,
    openers: String,
    edits: Vec<LineEdit>,
}

fn end_of(start: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos {
            line: start.line,
            col: start.col + text.len(),
        },
        Some(i) => Pos {
            line: start.line + text.matches('\n').count(),
            col: text.len() - i - 1,
        },
    }
}

fn sanitize(text: &str) -> String {
    text::normalize_newlines(text)
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}

impl Doc {
    pub fn new(text: &str) -> Self {
        let mut doc = Self {
            lines: vec![String::new()],
            anchor: Pos::default(),
            head: Pos::default(),
            goal: None,
            undo: Vec::new(),
            redo: Vec::new(),
            open: false,
            openers: "{".to_owned(),
            edits: Vec::new(),
        };
        doc.set_text(text);
        doc
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text::normalize_newlines(text)
            .split('\n')
            .map(str::to_owned)
            .collect();
        self.anchor = Pos::default();
        self.head = Pos::default();
        self.goal = None;
        self.undo.clear();
        self.redo.clear();
        self.open = false;
        self.edits.clear();
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> &str {
        &self.lines[i]
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn anchor(&self) -> Pos {
        self.anchor
    }

    pub fn head(&self) -> Pos {
        self.head
    }

    pub fn selection(&self) -> Range<Pos> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn set_indent_openers(&mut self, openers: &str) {
        self.openers = openers.to_owned();
    }

    pub fn take_line_edits(&mut self) -> Vec<LineEdit> {
        std::mem::take(&mut self.edits)
    }

    fn clamp(&self, pos: Pos) -> Pos {
        let line = pos.line.min(self.lines.len() - 1);
        let text = &self.lines[line];
        let mut col = pos.col.min(text.len());
        while !text.is_char_boundary(col) {
            col -= 1;
        }
        Pos { line, col }
    }

    fn line_end(&self, line: usize) -> Pos {
        Pos {
            line,
            col: self.lines[line].len(),
        }
    }

    fn doc_end(&self) -> Pos {
        self.line_end(self.lines.len() - 1)
    }

    /// Sets the selection from outside an edit: ends the current undo group.
    pub fn set_selection(&mut self, anchor: Pos, head: Pos) {
        self.anchor = self.clamp(anchor);
        self.head = self.clamp(head);
        self.goal = None;
        self.open = false;
    }

    pub fn text_in(&self, range: Range<Pos>) -> String {
        let (s, e) = (self.clamp(range.start), self.clamp(range.end));
        if s.line == e.line {
            return self.lines[s.line][s.col.min(e.col)..e.col].to_owned();
        }
        let mut out = self.lines[s.line][s.col..].to_owned();
        for l in &self.lines[s.line + 1..e.line] {
            out.push('\n');
            out.push_str(l);
        }
        out.push('\n');
        out.push_str(&self.lines[e.line][..e.col]);
        out
    }

    pub fn selected_text(&self) -> String {
        self.text_in(self.selection())
    }

    // ---- editing core ----

    fn raw_replace(&mut self, range: Range<Pos>, text: &str) -> Pos {
        let (s, e) = (range.start, range.end);
        let joined = format!(
            "{}{}{}",
            &self.lines[s.line][..s.col],
            text,
            &self.lines[e.line][e.col..]
        );
        let new: Vec<String> = joined.split('\n').map(str::to_owned).collect();
        self.edits.push(LineEdit {
            start_line: s.line,
            removed: e.line - s.line + 1,
            inserted: new.len(),
        });
        self.lines.splice(s.line..=e.line, new);
        end_of(s, text)
    }

    /// An undoable replacement; must run inside `edit`.
    fn splice(&mut self, range: Range<Pos>, text: &str) -> Pos {
        let removed = self.text_in(range.clone());
        if removed.is_empty() && text.is_empty() {
            return range.start;
        }
        let end = self.raw_replace(range.clone(), text);
        if let Some(group) = self.undo.last_mut() {
            group.recs.push(Rec {
                start: range.start,
                removed,
                inserted: text.to_owned(),
            });
        }
        end
    }

    fn set_caret(&mut self, pos: Pos) {
        self.anchor = pos;
        self.head = pos;
        self.goal = None;
    }

    fn edit(&mut self, kind: Kind, f: impl FnOnce(&mut Self)) {
        let before = (self.anchor, self.head);
        let coalesce = self.open
            && kind != Kind::Other
            && self
                .undo
                .last()
                .is_some_and(|g| g.kind == kind && g.after == before);
        if !coalesce {
            self.undo.push(Group {
                recs: Vec::new(),
                kind,
                before,
                after: before,
            });
        }
        let had = self.undo.last().map_or(0, |g| g.recs.len());
        f(self);
        let group = self.undo.last_mut().expect("group pushed above");
        if group.recs.len() == had {
            if !coalesce {
                self.undo.pop();
            }
            return;
        }
        group.after = (self.anchor, self.head);
        self.redo.clear();
        self.open = true;
    }

    pub fn replace_range(&mut self, range: Range<Pos>, text: &str) {
        let (s, e) = (self.clamp(range.start), self.clamp(range.end));
        let text = sanitize(text);
        self.edit(Kind::Other, |d| {
            let end = d.splice(s..e, &text);
            d.set_caret(end);
        });
    }

    pub fn insert(&mut self, text: &str) {
        let text = sanitize(text);
        let sel = self.selection();
        let typing = sel.start == sel.end && text.chars().count() == 1 && text != "\n";
        self.edit(if typing { Kind::Typing } else { Kind::Other }, |d| {
            let end = d.splice(sel, &text);
            d.set_caret(end);
        });
    }

    pub fn newline(&mut self) {
        let sel = self.selection();
        let prefix = self.lines[sel.start.line][..sel.start.col].to_owned();
        let suffix = self.lines[sel.end.line][sel.end.col..].to_owned();
        let indent: String = prefix
            .chars()
            .take_while(|&c| c == ' ' || c == '\t')
            .collect();
        let opener = prefix
            .trim_end()
            .chars()
            .last()
            .filter(|c| self.openers.contains(*c));
        let extra = match opener {
            Some(_) if indent.contains('\t') => "\t",
            Some(_) => INDENT,
            None => "",
        };
        let closer = opener.and_then(|o| match o {
            '{' => Some('}'),
            '(' => Some(')'),
            '[' => Some(']'),
            _ => None,
        });
        let split = closer.is_some_and(|c| suffix.trim_start().starts_with(c));
        let mut text = format!("\n{indent}{extra}");
        if split {
            text.push('\n');
            text.push_str(&indent);
        }
        self.edit(Kind::Other, |d| {
            d.splice(sel.clone(), &text);
            d.set_caret(Pos {
                line: sel.start.line + 1,
                col: indent.len() + extra.len(),
            });
        });
    }

    fn delete_selection(&mut self) -> bool {
        let sel = self.selection();
        if sel.start == sel.end {
            return false;
        }
        self.edit(Kind::Other, |d| {
            d.splice(sel.clone(), "");
            d.set_caret(sel.start);
        });
        true
    }

    fn delete_range(&mut self, kind: Kind, range: Range<Pos>) {
        self.edit(kind, |d| {
            d.splice(range.clone(), "");
            d.set_caret(range.start);
        });
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, col } = self.head;
        let start = if col > 0 {
            Pos {
                line,
                col: text::prev_grapheme(&self.lines[line], col),
            }
        } else if line > 0 {
            self.line_end(line - 1)
        } else {
            return;
        };
        self.delete_range(Kind::Backspace, start..self.head);
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, col } = self.head;
        let end = if col < self.lines[line].len() {
            Pos {
                line,
                col: text::next_grapheme(&self.lines[line], col),
            }
        } else if line + 1 < self.lines.len() {
            Pos {
                line: line + 1,
                col: 0,
            }
        } else {
            return;
        };
        self.delete_range(Kind::Delete, self.head..end);
    }

    pub fn delete_word_left(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, col } = self.head;
        let start = if col > 0 {
            Pos {
                line,
                col: text::prev_word_boundary(&self.lines[line], col),
            }
        } else if line > 0 {
            self.line_end(line - 1)
        } else {
            return;
        };
        self.delete_range(Kind::Other, start..self.head);
    }

    pub fn delete_word_right(&mut self) {
        if self.delete_selection() {
            return;
        }
        let Pos { line, col } = self.head;
        let end = if col < self.lines[line].len() {
            Pos {
                line,
                col: text::next_word_boundary(&self.lines[line], col),
            }
        } else if line + 1 < self.lines.len() {
            Pos {
                line: line + 1,
                col: 0,
            }
        } else {
            return;
        };
        self.delete_range(Kind::Other, self.head..end);
    }

    /// Lines an indent/outdent applies to: the selected ones, minus the last
    /// when the selection ends at its column 0.
    fn block_lines(&self) -> Range<usize> {
        let sel = self.selection();
        let last = if sel.end.col == 0 && sel.end.line > sel.start.line {
            sel.end.line - 1
        } else {
            sel.end.line
        };
        sel.start.line..last + 1
    }

    fn shift_endpoint(p: Pos, changed: &[(usize, isize)]) -> Pos {
        match changed.iter().find(|(l, _)| *l == p.line) {
            Some(&(_, delta)) if delta >= 0 && p.col > 0 => Pos {
                line: p.line,
                col: p.col + delta as usize,
            },
            Some(&(_, delta)) if delta < 0 => Pos {
                line: p.line,
                col: p.col.saturating_sub(delta.unsigned_abs()),
            },
            _ => p,
        }
    }

    pub fn indent(&mut self) {
        let sel = self.selection();
        if sel.start.line == sel.end.line {
            let line = &self.lines[sel.start.line];
            let col = text::display_col(line, sel.start.col, TAB_WIDTH);
            let pad = " ".repeat(TAB_WIDTH - col % TAB_WIDTH);
            self.edit(Kind::Other, |d| {
                let end = d.splice(sel, &pad);
                d.set_caret(end);
            });
            return;
        }
        let lines = self.block_lines();
        let (anchor, head) = (self.anchor, self.head);
        self.edit(Kind::Other, |d| {
            let mut changed = Vec::new();
            for l in lines {
                if d.lines[l].is_empty() {
                    continue;
                }
                d.splice(Pos { line: l, col: 0 }..Pos { line: l, col: 0 }, INDENT);
                changed.push((l, INDENT.len() as isize));
            }
            d.anchor = Self::shift_endpoint(anchor, &changed);
            d.head = Self::shift_endpoint(head, &changed);
        });
    }

    pub fn outdent(&mut self) {
        let lines = self.block_lines();
        let (anchor, head) = (self.anchor, self.head);
        self.edit(Kind::Other, |d| {
            let mut changed = Vec::new();
            for l in lines {
                let n = outdent_bytes(&d.lines[l]);
                if n > 0 {
                    d.splice(Pos { line: l, col: 0 }..Pos { line: l, col: n }, "");
                    changed.push((l, -(n as isize)));
                }
            }
            d.anchor = Self::shift_endpoint(anchor, &changed);
            d.head = Self::shift_endpoint(head, &changed);
        });
    }

    pub fn duplicate_line(&mut self) {
        let lines = self.block_lines();
        let (anchor, head) = (self.anchor, self.head);
        let shift = lines.len();
        let copy = self.lines[lines.clone()].join("\n");
        let at = self.line_end(lines.end - 1);
        self.edit(Kind::Other, |d| {
            d.splice(at..at, &format!("\n{copy}"));
            d.anchor = Pos {
                line: anchor.line + shift,
                col: anchor.col,
            };
            d.head = Pos {
                line: head.line + shift,
                col: head.col,
            };
        });
    }

    // ---- movement ----

    fn move_to(&mut self, pos: Pos, extend: bool, keep_goal: bool) {
        self.head = pos;
        if !extend {
            self.anchor = pos;
        }
        if !keep_goal {
            self.goal = None;
        }
        self.open = false;
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && self.anchor != self.head {
            return self.move_to(self.selection().start, false, false);
        }
        let Pos { line, col } = self.head;
        let to = if col > 0 {
            Pos {
                line,
                col: text::prev_grapheme(&self.lines[line], col),
            }
        } else if line > 0 {
            self.line_end(line - 1)
        } else {
            self.head
        };
        self.move_to(to, extend, false);
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && self.anchor != self.head {
            return self.move_to(self.selection().end, false, false);
        }
        let Pos { line, col } = self.head;
        let to = if col < self.lines[line].len() {
            Pos {
                line,
                col: text::next_grapheme(&self.lines[line], col),
            }
        } else if line + 1 < self.lines.len() {
            Pos {
                line: line + 1,
                col: 0,
            }
        } else {
            self.head
        };
        self.move_to(to, extend, false);
    }

    pub fn move_up(&mut self, extend: bool) {
        self.move_lines(-1, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.move_lines(1, extend);
    }

    /// Moves the caret `delta` lines, keeping the goal column. At the first or
    /// last line it goes to the start or end of the document instead.
    pub fn move_lines(&mut self, delta: isize, extend: bool) {
        let from = self.head.line as isize;
        let to = from + delta;
        if to < 0 {
            return self.move_to(Pos::default(), extend, false);
        }
        if to >= self.lines.len() as isize {
            return self.move_to(self.doc_end(), extend, false);
        }
        let goal = self.goal.unwrap_or_else(|| {
            text::display_col(&self.lines[self.head.line], self.head.col, TAB_WIDTH)
        });
        let line = to as usize;
        let col = text::byte_at_col(&self.lines[line], goal, TAB_WIDTH);
        self.move_to(Pos { line, col }, extend, true);
        self.goal = Some(goal);
    }

    pub fn move_word_left(&mut self, extend: bool) {
        let Pos { line, col } = self.head;
        let to = if col > 0 {
            Pos {
                line,
                col: text::prev_word_boundary(&self.lines[line], col),
            }
        } else if line > 0 {
            self.line_end(line - 1)
        } else {
            self.head
        };
        self.move_to(to, extend, false);
    }

    pub fn move_word_right(&mut self, extend: bool) {
        let Pos { line, col } = self.head;
        let to = if col < self.lines[line].len() {
            Pos {
                line,
                col: text::next_word_boundary(&self.lines[line], col),
            }
        } else if line + 1 < self.lines.len() {
            Pos {
                line: line + 1,
                col: 0,
            }
        } else {
            self.head
        };
        self.move_to(to, extend, false);
    }

    /// Smart home: first non-blank character, then column 0, then back.
    pub fn move_home(&mut self, extend: bool) {
        let line = self.head.line;
        let first = text::first_nonblank(&self.lines[line]);
        let col = if self.head.col == first { 0 } else { first };
        self.move_to(Pos { line, col }, extend, false);
    }

    pub fn move_end(&mut self, extend: bool) {
        self.move_to(self.line_end(self.head.line), extend, false);
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        self.move_to(Pos::default(), extend, false);
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        self.move_to(self.doc_end(), extend, false);
    }

    pub fn select_all(&mut self) {
        self.set_selection(Pos::default(), self.doc_end());
    }

    pub fn select_word_at(&mut self, pos: Pos) {
        let pos = self.clamp(pos);
        let r = text::word_range_at(&self.lines[pos.line], pos.col);
        self.set_selection(
            Pos {
                line: pos.line,
                col: r.start,
            },
            Pos {
                line: pos.line,
                col: r.end,
            },
        );
    }

    pub fn select_line(&mut self, line: usize) {
        let line = line.min(self.lines.len() - 1);
        let end = if line + 1 < self.lines.len() {
            Pos {
                line: line + 1,
                col: 0,
            }
        } else {
            self.line_end(line)
        };
        self.set_selection(Pos { line, col: 0 }, end);
    }

    // ---- history ----

    pub fn undo(&mut self) -> bool {
        let Some(group) = self.undo.pop() else {
            return false;
        };
        for rec in group.recs.iter().rev() {
            let end = end_of(rec.start, &rec.inserted);
            self.raw_replace(rec.start..end, &rec.removed);
        }
        (self.anchor, self.head) = group.before;
        self.goal = None;
        self.open = false;
        self.redo.push(group);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(group) = self.redo.pop() else {
            return false;
        };
        for rec in &group.recs {
            let end = end_of(rec.start, &rec.removed);
            self.raw_replace(rec.start..end, &rec.inserted);
        }
        (self.anchor, self.head) = group.after;
        self.goal = None;
        self.open = false;
        self.undo.push(group);
        true
    }

    // ---- UTF-16 (platform IME offsets span the whole document) ----

    pub fn pos_to_utf16(&self, pos: Pos) -> usize {
        let pos = self.clamp(pos);
        let before: usize = self.lines[..pos.line]
            .iter()
            .map(|l| text::utf16_len(l) + 1)
            .sum();
        before + text::utf16_len(&self.lines[pos.line][..pos.col])
    }

    pub fn utf16_to_pos(&self, offset: usize) -> Pos {
        let mut rest = offset;
        for (line, l) in self.lines.iter().enumerate() {
            let len = text::utf16_len(l);
            if rest <= len {
                let mut units = 0;
                for (col, c) in l.char_indices() {
                    if units + c.len_utf16() > rest {
                        return Pos { line, col };
                    }
                    units += c.len_utf16();
                }
                return Pos { line, col: l.len() };
            }
            rest -= len + 1;
        }
        self.doc_end()
    }
}

/// Bytes of leading whitespace one outdent removes: up to four spaces, or a
/// tab (which reaches the next stop by itself).
fn outdent_bytes(line: &str) -> usize {
    let mut cols = 0;
    let mut bytes = 0;
    for c in line.chars() {
        match c {
            ' ' => {
                cols += 1;
                bytes += 1;
                if cols == TAB_WIDTH {
                    break;
                }
            }
            '\t' => {
                bytes += 1;
                break;
            }
            _ => break,
        }
    }
    bytes
}

#[cfg(test)]
mod tests;
