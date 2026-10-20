//! What the code views share for H2: a line selection anchored at a position
//! (a line index, or a byte offset in the hex view) that runs to the caret,
//! and the text Copy puts on the clipboard. No UI, no host.

use std::ops::RangeInclusive;

/// The fixed end of a selection; the caret is the moving end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Anchor(Option<usize>);

impl Anchor {
    /// Starts a selection at `cursor` unless one is already running
    /// (Shift+arrow, Shift+click, the start of a drag).
    pub fn begin(&mut self, cursor: usize) {
        self.0.get_or_insert(cursor);
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }

    /// The selected positions, lowest first; `None` when nothing is selected
    /// (no anchor, or the anchor is the caret's own position).
    pub fn range(&self, cursor: usize) -> Option<RangeInclusive<usize>> {
        let a = self.0?;
        (a != cursor).then(|| a.min(cursor)..=a.max(cursor))
    }

    pub fn active(&self, cursor: usize) -> bool {
        self.range(cursor).is_some()
    }

    /// The buffer under the anchor grew or shrank at its front by `delta`.
    pub fn shift(&mut self, delta: isize) {
        if let Some(a) = self.0 {
            self.0 = Some((a as isize + delta).max(0) as usize);
        }
    }
}

/// Drag-select past the edge of a view: -1 above the top, 1 below the
/// bottom, 0 inside (or beside it). The view then scrolls that way by a line
/// per tick while the button stays down.
pub fn edge_direction(y: f32, top: f32, bottom: f32) -> isize {
    if y < top {
        -1
    } else if y > bottom {
        1
    } else {
        0
    }
}

/// One line as Copy sees it: its address text (the listing's gutter) and its text.
pub struct CopyLine<'a> {
    pub addr: Option<&'a str>,
    pub text: &'a str,
}

/// The lines as plain text: trailing blanks dropped, the address in front
/// when `with_addr`.
pub fn lines_text(lines: &[CopyLine], with_addr: bool) -> String {
    lines
        .iter()
        .map(|l| match l.addr.filter(|_| with_addr) {
            Some(a) => format!("{a}  {}", l.text.trim_end()),
            None => l.text.trim_end().to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BytesFormat {
    Hex,
    CArray,
    Python,
}

/// Bytes as `48 8B 05`, a C array, or a Python bytes literal.
pub fn bytes_text(bytes: &[u8], format: BytesFormat) -> String {
    match format {
        BytesFormat::Hex => bytes
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
        BytesFormat::CArray => {
            let rows: Vec<String> = bytes
                .chunks(16)
                .map(|c| {
                    let cells: Vec<String> = c.iter().map(|b| format!("0x{b:02X}")).collect();
                    format!("    {},", cells.join(", "))
                })
                .collect();
            format!("unsigned char data[] = {{\n{}\n}};", rows.join("\n"))
        }
        BytesFormat::Python => {
            let body: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
            format!("b\"{body}\"")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_runs_from_the_anchor_to_the_caret_in_either_direction() {
        let mut a = Anchor::default();
        assert_eq!(a.range(5), None);
        a.begin(5);
        assert_eq!(a.range(5), None, "an anchor on the caret selects nothing");
        assert_eq!(a.range(8), Some(5..=8));
        assert_eq!(a.range(2), Some(2..=5));
    }

    #[test]
    fn beginning_again_keeps_the_first_anchor_and_clear_drops_it() {
        let mut a = Anchor::default();
        a.begin(5);
        a.begin(9);
        assert_eq!(a.range(7), Some(5..=7));
        a.clear();
        assert_eq!(a.range(7), None);
    }

    #[test]
    fn the_anchor_follows_rows_added_or_trimmed_at_the_front() {
        let mut a = Anchor::default();
        a.begin(10);
        a.shift(200);
        assert_eq!(a.range(215), Some(210..=215));
        a.shift(-500);
        assert_eq!(a.range(3), Some(0..=3), "never below the first row");
        // No anchor: nothing to move.
        let mut none = Anchor::default();
        none.shift(5);
        assert_eq!(none.range(1), None);
    }

    #[test]
    fn dragging_past_an_edge_scrolls_toward_it() {
        assert_eq!(edge_direction(5., 10., 100.), -1);
        assert_eq!(edge_direction(150., 10., 100.), 1);
        assert_eq!(
            edge_direction(10., 10., 100.),
            0,
            "the edge itself is inside"
        );
        assert_eq!(edge_direction(100., 10., 100.), 0);
    }

    #[test]
    fn copied_lines_keep_their_indent_and_lose_trailing_blanks() {
        let lines = [
            CopyLine {
                addr: Some(".text:0000000140001000"),
                text: "    push    rbp   ",
            },
            CopyLine {
                addr: Some(".text:0000000140001001"),
                text: "    mov     rbp, rsp",
            },
        ];
        assert_eq!(
            lines_text(&lines, false),
            "    push    rbp\n    mov     rbp, rsp"
        );
        assert_eq!(
            lines_text(&lines, true),
            ".text:0000000140001000      push    rbp\n.text:0000000140001001      mov     rbp, rsp"
        );
    }

    #[test]
    fn lines_without_an_address_copy_without_one_even_when_asked() {
        let lines = [CopyLine {
            addr: None,
            text: "int x;",
        }];
        assert_eq!(lines_text(&lines, true), "int x;");
    }

    #[test]
    fn bytes_copy_as_hex_c_array_and_python() {
        let b = [0x48, 0x8B, 0x05, 0x00];
        assert_eq!(bytes_text(&b, BytesFormat::Hex), "48 8B 05 00");
        assert_eq!(
            bytes_text(&b, BytesFormat::Python),
            "b\"\\x48\\x8b\\x05\\x00\""
        );
        assert_eq!(
            bytes_text(&b, BytesFormat::CArray),
            "unsigned char data[] = {\n    0x48, 0x8B, 0x05, 0x00,\n};"
        );
    }

    #[test]
    fn a_long_c_array_wraps_at_sixteen_bytes() {
        let b: Vec<u8> = (0..20).collect();
        let text = bytes_text(&b, BytesFormat::CArray);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].matches("0x").count(), 16);
        assert_eq!(rows[2].matches("0x").count(), 4);
        assert_eq!(bytes_text(&[], BytesFormat::Hex), "");
    }
}
