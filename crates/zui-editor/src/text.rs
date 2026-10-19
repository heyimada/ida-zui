// Pure string helpers: newline normalisation, word classes, tab expansion and
// UTF-16 lengths. No gpui types.
use std::borrow::Cow;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub const TAB_WIDTH: usize = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    Space,
    Word,
    Punct,
}

pub fn normalize_newlines(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_owned();
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// Start of the word before `col`: skip whitespace, then one run of a class.
pub fn prev_word_boundary(line: &str, col: usize) -> usize {
    let mut chars = line[..col].char_indices().rev().peekable();
    let mut at = col;
    while let Some(&(i, c)) = chars.peek() {
        if class(c) != CharClass::Space {
            break;
        }
        at = i;
        chars.next();
    }
    let Some(&(_, first)) = chars.peek() else {
        return at;
    };
    let want = class(first);
    for (i, c) in chars {
        if class(c) != want {
            break;
        }
        at = i;
    }
    at
}

/// End of the word after `col`: skip whitespace, then one run of a class.
pub fn next_word_boundary(line: &str, col: usize) -> usize {
    let mut chars = line[col..].char_indices().peekable();
    let mut at = col;
    while let Some(&(i, c)) = chars.peek() {
        if class(c) != CharClass::Space {
            break;
        }
        at = col + i + c.len_utf8();
        chars.next();
    }
    let Some(&(_, first)) = chars.peek() else {
        return at;
    };
    let want = class(first);
    for (i, c) in chars {
        if class(c) != want {
            break;
        }
        at = col + i + c.len_utf8();
    }
    at
}

/// The run of equal-class characters under `col` (the one after it, or the one
/// before it at the end of the line).
pub fn word_range_at(line: &str, col: usize) -> Range<usize> {
    let col = col.min(line.len());
    let probe = line[col..]
        .chars()
        .next()
        .or_else(|| line[..col].chars().next_back());
    let Some(probe) = probe else { return col..col };
    let want = class(probe);
    let anchor = if col < line.len() {
        col
    } else {
        col - probe.len_utf8()
    };
    let start = line[..anchor]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| class(c) == want)
        .last()
        .map_or(anchor, |(i, _)| i);
    let end = line[anchor..]
        .char_indices()
        .take_while(|&(_, c)| class(c) == want)
        .last()
        .map_or(anchor, |(i, c)| anchor + i + c.len_utf8());
    start..end
}

pub fn first_nonblank(line: &str) -> usize {
    line.char_indices()
        .find(|&(_, c)| !c.is_whitespace())
        .map_or(line.len(), |(i, _)| i)
}

pub fn prev_grapheme(line: &str, col: usize) -> usize {
    line.grapheme_indices(true)
        .rev()
        .find_map(|(i, _)| (i < col).then_some(i))
        .unwrap_or(0)
}

pub fn next_grapheme(line: &str, col: usize) -> usize {
    line.grapheme_indices(true)
        .find_map(|(i, _)| (i > col).then_some(i))
        .unwrap_or(line.len())
}

/// Replaces tabs with spaces up to the next multiple of `tab`.
pub fn expand_tabs(line: &str, tab: usize) -> Cow<'_, str> {
    if !line.contains('\t') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = tab - col % tab;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    Cow::Owned(out)
}

/// Byte offset in the tab-expanded line for a byte offset in the raw line.
pub fn to_display(line: &str, byte: usize, tab: usize) -> usize {
    if !line.contains('\t') {
        return byte;
    }
    let mut disp = 0;
    let mut col = 0;
    for (i, c) in line.char_indices() {
        if i >= byte {
            break;
        }
        if c == '\t' {
            let n = tab - col % tab;
            disp += n;
            col += n;
        } else {
            disp += c.len_utf8();
            col += 1;
        }
    }
    disp
}

/// Raw byte offset for a byte offset in the expanded line. Inside a tab the
/// nearer edge wins.
pub fn from_display(line: &str, disp: usize, tab: usize) -> usize {
    if !line.contains('\t') {
        return disp.min(line.len());
    }
    let mut at = 0;
    let mut col = 0;
    for (i, c) in line.char_indices() {
        let width = if c == '\t' {
            tab - col % tab
        } else {
            c.len_utf8()
        };
        if disp < at + width {
            return if disp - at > width / 2 {
                i + c.len_utf8()
            } else {
                i
            };
        }
        at += width;
        col += if c == '\t' { width } else { 1 };
    }
    line.len()
}

/// Character column (tabs advance to the stop) of a byte offset.
pub fn display_col(line: &str, byte: usize, tab: usize) -> usize {
    let mut col = 0;
    for (i, c) in line.char_indices() {
        if i >= byte {
            break;
        }
        col += if c == '\t' { tab - col % tab } else { 1 };
    }
    col
}

/// Byte offset for a character column; inside a tab the nearer edge wins.
pub fn byte_at_col(line: &str, want: usize, tab: usize) -> usize {
    let mut col = 0;
    for (i, c) in line.char_indices() {
        if col >= want {
            return i;
        }
        let width = if c == '\t' { tab - col % tab } else { 1 };
        if want < col + width {
            return if want - col > width / 2 {
                i + c.len_utf8()
            } else {
                i
            };
        }
        col += width;
    }
    line.len()
}

pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests;
