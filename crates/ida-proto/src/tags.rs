//! Parser for IDA color-tagged lines (`lines.hpp`).
//!
//! IDA renders every listing and pseudocode line as text interleaved with
//! escape sequences: `\x01 c` opens color `c`, `\x02 c` closes it, `\x03 x`
//! quotes a literal `x`, `\x04` toggles inverse video. `\x01 \x28` is followed
//! by a 16-digit hex anchor (an address in the listing, a ctree item index in
//! pseudocode) and has no closing tag. Codes 0x29..=0x30 bracket operands.

use std::ops::Range;

pub const COLOR_ON: u8 = 1;
pub const COLOR_OFF: u8 = 2;
pub const COLOR_ESC: u8 = 3;
pub const COLOR_INV: u8 = 4;
pub const COLOR_ADDR: u8 = 0x28;
pub const COLOR_ADDR_SIZE: usize = 16;
const COLOR_OPND1: u8 = 0x29;
const COLOR_OPND8: u8 = 0x30;

/// IDA syntax color codes (subset that the UI themes individually).
pub mod color {
    pub const DEFAULT: u8 = 0x01;
    pub const REGCMT: u8 = 0x02;
    pub const RPTCMT: u8 = 0x03;
    pub const AUTOCMT: u8 = 0x04;
    pub const INSN: u8 = 0x05;
    pub const DATNAME: u8 = 0x06;
    pub const DNAME: u8 = 0x07;
    pub const DEMNAME: u8 = 0x08;
    pub const SYMBOL: u8 = 0x09;
    pub const CHAR: u8 = 0x0A;
    pub const STRING: u8 = 0x0B;
    pub const NUMBER: u8 = 0x0C;
    pub const VOIDOP: u8 = 0x0D;
    pub const CREF: u8 = 0x0E;
    pub const DREF: u8 = 0x0F;
    pub const CREFTAIL: u8 = 0x10;
    pub const DREFTAIL: u8 = 0x11;
    pub const ERROR: u8 = 0x12;
    pub const PREFIX: u8 = 0x13;
    pub const BINPREF: u8 = 0x14;
    pub const EXTRA: u8 = 0x15;
    pub const ALTOP: u8 = 0x16;
    pub const HIDNAME: u8 = 0x17;
    pub const LIBNAME: u8 = 0x18;
    pub const LOCNAME: u8 = 0x19;
    pub const CODNAME: u8 = 0x1A;
    pub const ASMDIR: u8 = 0x1B;
    pub const MACRO: u8 = 0x1C;
    pub const DSTR: u8 = 0x1D;
    pub const DCHAR: u8 = 0x1E;
    pub const DNUM: u8 = 0x1F;
    pub const KEYWORD: u8 = 0x20;
    pub const REG: u8 = 0x21;
    pub const IMPNAME: u8 = 0x22;
    pub const SEGNAME: u8 = 0x23;
    pub const UNKNAME: u8 = 0x24;
    pub const CNAME: u8 = 0x25;
    pub const UNAME: u8 = 0x26;
    pub const COLLAPSED: u8 = 0x27;

    /// Colors whose text names something you can navigate to.
    pub fn is_name(c: u8) -> bool {
        matches!(
            c,
            DATNAME
                | DNAME
                | DEMNAME
                | CREF
                | DREF
                | LIBNAME
                | CODNAME
                | IMPNAME
                | UNKNAME
                | CNAME
                | UNAME
                | LOCNAME
                | HIDNAME
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Byte range into [`Line::text`].
    pub range: Range<usize>,
    pub color: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Anchor {
    /// Byte offset into [`Line::text`] where the anchored item starts.
    pub pos: usize,
    pub value: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Operand {
    pub range: Range<usize>,
    /// Zero-based operand number.
    pub n: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    /// Contiguous, non-overlapping runs covering all of `text`.
    pub runs: Vec<Run>,
    pub anchors: Vec<Anchor>,
    pub operands: Vec<Operand>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let runs = if text.is_empty() {
            Vec::new()
        } else {
            vec![Run {
                range: 0..text.len(),
                color: color::DEFAULT,
            }]
        };
        Self {
            text,
            runs,
            ..Default::default()
        }
    }

    /// Color of the byte at `pos`, if any.
    pub fn color_at(&self, pos: usize) -> Option<u8> {
        self.runs
            .iter()
            .find(|r| r.range.contains(&pos))
            .map(|r| r.color)
    }

    /// The run containing `pos`.
    pub fn run_at(&self, pos: usize) -> Option<&Run> {
        self.runs.iter().find(|r| r.range.contains(&pos))
    }

    /// The anchor governing `pos`: the last one at or before it.
    pub fn anchor_at(&self, pos: usize) -> Option<u64> {
        self.anchors
            .iter()
            .take_while(|a| a.pos <= pos)
            .last()
            .map(|a| a.value)
    }

    /// The identifier-like word around `pos` (IDA's "highlight" semantics).
    pub fn word_at(&self, pos: usize) -> Option<Range<usize>> {
        let bytes = self.text.as_bytes();
        if pos >= bytes.len() || !is_word_byte(bytes[pos]) {
            return None;
        }
        let mut start = pos;
        while start > 0 && is_word_byte(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = pos;
        while end < bytes.len() && is_word_byte(bytes[end]) {
            end += 1;
        }
        Some(start..end)
    }
}

impl Line {
    /// Number of leading spaces.
    pub fn indent(&self) -> usize {
        self.text.bytes().take_while(|&b| b == b' ').count()
    }

    /// Whether this is an instruction line: not blank, not a comment and not
    /// a bare label.
    pub fn is_instruction(&self) -> bool {
        let t = self.text.trim();
        !t.is_empty()
            && !t.starts_with(';')
            && !(t.ends_with(':') && !t.contains(' '))
            && !self.is_frame_decl()
    }

    /// The `name proc near` header and the `var_8 = qword ptr -8` frame
    /// lines of an entry block: flush left, unlike the code under them.
    pub fn is_frame_decl(&self) -> bool {
        let t = self.text.trim();
        t.contains(" proc ") || t.ends_with(" endp") || (t.contains(" = ") && t.contains(" ptr "))
    }

    /// Removes `n` leading spaces (at most the line's own indent) and moves
    /// runs, anchors and operands with the text.
    pub fn trim_indent(&mut self, n: usize) {
        let n = n.min(self.indent());
        if n == 0 {
            return;
        }
        self.text.drain(..n);
        let shift = |r: &Range<usize>| r.start.saturating_sub(n)..r.end.saturating_sub(n);
        self.runs = self
            .runs
            .iter()
            .map(|r| Run {
                range: shift(&r.range),
                color: r.color,
            })
            .filter(|r| !r.range.is_empty())
            .collect();
        for a in &mut self.anchors {
            a.pos = a.pos.saturating_sub(n);
        }
        self.operands = self
            .operands
            .iter()
            .map(|o| Operand {
                range: shift(&o.range),
                n: o.n,
            })
            .filter(|o| !o.range.is_empty())
            .collect();
    }
}

/// Drops the indent that every instruction of a graph block shares, so the
/// block is as narrow as its text. Labels and comments do not set the amount
/// but lose as much as their own indent allows.
pub fn compact_block(lines: &mut [Line]) {
    let Some(common) = lines
        .iter()
        .filter(|l| l.is_instruction())
        .map(Line::indent)
        .min()
    else {
        return;
    };
    for l in lines {
        l.trim_indent(common);
    }
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$' | b'?' | b'@' | b'.' | b':') || b >= 0x80
}

/// Parses one tagged line. Never panics; malformed tags are dropped.
pub fn parse(tagged: &str) -> Line {
    let bytes = tagged.as_bytes();
    let mut text = Vec::with_capacity(bytes.len());
    let mut colors: Vec<u8> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut anchors = Vec::new();
    let mut operands = Vec::new();
    let mut open_operands: Vec<(u8, usize)> = Vec::new();

    let mut i = 0;
    let push_byte = |b: u8, text: &mut Vec<u8>, runs: &mut Vec<Run>, colors: &Vec<u8>| {
        let color = colors.last().copied().unwrap_or(color::DEFAULT);
        let pos = text.len();
        text.push(b);
        match runs.last_mut() {
            Some(run) if run.color == color && run.range.end == pos => run.range.end = pos + 1,
            _ => runs.push(Run {
                range: pos..pos + 1,
                color,
            }),
        }
    };

    while i < bytes.len() {
        let b = bytes[i];
        match b {
            COLOR_ON => {
                let Some(&code) = bytes.get(i + 1) else { break };
                i += 2;
                if code == COLOR_ADDR {
                    let end = (i + COLOR_ADDR_SIZE).min(bytes.len());
                    let digits = &bytes[i..end];
                    if digits.len() == COLOR_ADDR_SIZE
                        && let Ok(s) = std::str::from_utf8(digits)
                        && let Ok(value) = u64::from_str_radix(s, 16)
                    {
                        anchors.push(Anchor {
                            pos: text.len(),
                            value,
                        });
                    }
                    i = end;
                } else if (COLOR_OPND1..=COLOR_OPND8).contains(&code) {
                    open_operands.push((code - COLOR_OPND1, text.len()));
                } else {
                    colors.push(code);
                }
            }
            COLOR_OFF => {
                let Some(&code) = bytes.get(i + 1) else { break };
                i += 2;
                if (COLOR_OPND1..=COLOR_OPND8).contains(&code) {
                    let n = code - COLOR_OPND1;
                    if let Some(at) = open_operands.iter().rposition(|&(m, _)| m == n) {
                        let (_, start) = open_operands.remove(at);
                        operands.push(Operand {
                            range: start..text.len(),
                            n,
                        });
                    }
                } else if let Some(at) = colors.iter().rposition(|&c| c == code) {
                    colors.truncate(at);
                }
            }
            COLOR_ESC => {
                if let Some(&lit) = bytes.get(i + 1) {
                    push_byte(lit, &mut text, &mut runs, &colors);
                }
                i += 2;
            }
            COLOR_INV => i += 1,
            _ => {
                push_byte(b, &mut text, &mut runs, &colors);
                i += 1;
            }
        }
    }

    // Tags are ASCII, so stripping them keeps UTF-8 intact except when a quoted
    // byte or a truncated sequence split a code point; repair those lossily.
    let text = match String::from_utf8(text) {
        Ok(s) => s,
        Err(e) => {
            let s = String::from_utf8_lossy(e.as_bytes()).into_owned();
            return Line::plain(s);
        }
    };
    operands.sort_by_key(|o| o.range.start);
    Line {
        text,
        runs,
        anchors,
        operands,
    }
}

/// Strips all tags, keeping only visible text.
pub fn strip(tagged: &str) -> String {
    parse(tagged).text
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from IDA `generate_disasm_line` on find.exe.
    const SUB_RSP: &str = "\u{1}\u{5}sub\u{2}\u{5}     \u{1})\u{1}!rsp\u{2}!\u{2})\u{1}\t,\u{2}\t \u{1}*\u{1}\u{c}28h\u{2}\u{c}\u{2}*";
    const MOV_ARG: &str = "\u{1}\u{5}mov\u{2}\u{5}     \u{1})\u{1}\t[\u{2}\t\u{1}!rsp\u{2}!\u{1}\t+\u{2}\t\u{1}\u{19}arg_0\u{2}\u{19}\u{1}\t]\u{2}\t\u{2})\u{1}\t,\u{2}\t \u{1}*\u{1}!rbx\u{2}!\u{2}*";

    fn colored(line: &Line, needle: &str) -> u8 {
        let pos = line.text.find(needle).expect(needle);
        line.color_at(pos).unwrap()
    }

    #[test]
    fn real_disasm_line_text_and_colors() {
        let l = parse(SUB_RSP);
        assert_eq!(l.text, "sub     rsp, 28h");
        assert_eq!(colored(&l, "sub"), color::INSN);
        assert_eq!(colored(&l, "rsp"), color::REG);
        assert_eq!(colored(&l, ","), color::SYMBOL);
        assert_eq!(colored(&l, "28h"), color::NUMBER);
        assert_eq!(colored(&l, "     "), color::DEFAULT);
    }

    #[test]
    fn real_disasm_line_operands() {
        let l = parse(MOV_ARG);
        assert_eq!(l.text, "mov     [rsp+arg_0], rbx");
        let ops: Vec<_> = l
            .operands
            .iter()
            .map(|o| (&l.text[o.range.clone()], o.n))
            .collect();
        assert_eq!(ops, vec![("[rsp+arg_0]", 0), ("rbx", 1)]);
        assert_eq!(colored(&l, "arg_0"), color::LOCNAME);
    }

    #[test]
    fn runs_cover_text_contiguously() {
        for src in [SUB_RSP, MOV_ARG] {
            let l = parse(src);
            let mut at = 0;
            for r in &l.runs {
                assert_eq!(r.range.start, at);
                assert!(r.range.end > r.range.start);
                at = r.range.end;
            }
            assert_eq!(at, l.text.len());
        }
    }

    #[test]
    fn address_anchor_marks_following_text() {
        let src = "x = \u{1}(00000000000000041\u{1}\u{19}v1\u{2}\u{19};";
        let l = parse(src);
        assert_eq!(l.text, "x = 1v1;");
        assert_eq!(l.anchors, vec![Anchor { pos: 4, value: 4 }]);
        assert_eq!(l.anchor_at(0), None);
        assert_eq!(l.anchor_at(5), Some(4));
    }

    #[test]
    fn full_width_64bit_anchor() {
        let l = parse("\u{1}(FFFFFFFFFFFFFFFFa");
        assert_eq!(l.anchors[0].value, u64::MAX);
        assert_eq!(l.text, "a");
    }

    #[test]
    fn truncated_anchor_is_dropped_without_eating_past_end() {
        let l = parse("ab\u{1}(0000");
        assert_eq!(l.text, "ab");
        assert!(l.anchors.is_empty());
    }

    #[test]
    fn escape_quotes_control_bytes() {
        let l = parse("a\u{3}\u{1}b");
        assert_eq!(l.text, "a\u{1}b");
    }

    #[test]
    fn inverse_toggle_is_invisible() {
        assert_eq!(parse("\u{4}abc\u{4}").text, "abc");
    }

    #[test]
    fn unbalanced_off_is_ignored_and_closes_only_its_color() {
        let l = parse("\u{1}\u{5}a\u{1}\u{c}b\u{2}\u{5}c\u{2}\u{21}d");
        // Closing INSN (outer) also discards NUMBER pushed above it.
        assert_eq!(l.text, "abcd");
        assert_eq!(l.color_at(0), Some(color::INSN));
        assert_eq!(l.color_at(1), Some(color::NUMBER));
        assert_eq!(l.color_at(2), Some(color::DEFAULT));
        assert_eq!(l.color_at(3), Some(color::DEFAULT));
    }

    #[test]
    fn dangling_tag_bytes_at_end() {
        assert_eq!(parse("abc\u{1}").text, "abc");
        assert_eq!(parse("abc\u{2}").text, "abc");
        assert_eq!(parse("abc\u{3}").text, "abc");
    }

    #[test]
    fn utf8_passes_through() {
        let l = parse("\u{1}\u{b}\"Grüße – 東京\"\u{2}\u{b}");
        assert_eq!(l.text, "\"Grüße – 東京\"");
        assert_eq!(l.color_at(1), Some(color::STRING));
    }

    #[test]
    fn word_at_finds_identifiers() {
        let l = parse(MOV_ARG);
        let p = l.text.find("arg_0").unwrap() + 2;
        let w = l.word_at(p).unwrap();
        assert_eq!(&l.text[w], "arg_0");
        assert_eq!(l.word_at(l.text.find(',').unwrap()), None);
    }

    // Graph blocks drop the indent every instruction shares.
    fn insn(indent: usize, rest: &str) -> Line {
        let mut l = Line::plain(format!("{}{rest}", " ".repeat(indent)));
        // Colour the part after the indent so runs, anchors and operands have something to shift.
        l.runs = vec![
            Run {
                range: 0..indent,
                color: color::DEFAULT,
            },
            Run {
                range: indent..l.text.len(),
                color: color::INSN,
            },
        ];
        l.anchors = vec![Anchor {
            pos: indent,
            value: 7,
        }];
        l.operands = vec![Operand {
            range: indent..l.text.len(),
            n: 0,
        }];
        l
    }

    #[test]
    fn compact_block_removes_the_shared_instruction_indent() {
        let mut b = vec![insn(16, "mov     rax, 1"), insn(16, "ret")];
        compact_block(&mut b);
        assert_eq!(b[0].text, "mov     rax, 1");
        assert_eq!(b[1].text, "ret");
        // Shifted metadata stays on the same characters.
        assert_eq!(
            b[0].runs,
            vec![Run {
                range: 0..14,
                color: color::INSN
            }]
        );
        assert_eq!(b[0].anchors, vec![Anchor { pos: 0, value: 7 }]);
        assert_eq!(b[0].operands, vec![Operand { range: 0..14, n: 0 }]);
    }

    #[test]
    fn compact_block_keeps_relative_indent_between_instructions() {
        let mut b = vec![insn(16, "mov     rax, 1"), insn(20, "nop")];
        compact_block(&mut b);
        assert_eq!(b[0].text, "mov     rax, 1");
        assert_eq!(b[1].text, "    nop");
    }

    #[test]
    fn compact_block_does_not_let_labels_set_the_amount() {
        let mut b = vec![Line::plain("loc_1400:"), insn(16, "ret")];
        compact_block(&mut b);
        assert_eq!(b[0].text, "loc_1400:");
        assert_eq!(b[1].text, "ret");
    }

    #[test]
    fn compact_block_trims_comments_and_blank_lines_by_at_most_their_own_indent() {
        let mut b = vec![
            insn(16, "ret"),
            Line::plain("; a note"),
            Line::plain("      ; deep note"),
            Line::plain("  "),
            Line::plain(""),
        ];
        compact_block(&mut b);
        assert_eq!(b[1].text, "; a note");
        assert_eq!(b[2].text, "; deep note");
        assert_eq!(b[3].text, "");
        assert_eq!(b[4].text, "");
        assert_eq!(b[3].runs, Vec::new());
    }

    #[test]
    fn compact_block_clips_a_run_that_spans_the_cut() {
        // One run covers both the indent and the text.
        let mut l = Line::plain("        mov");
        let mut b = vec![l.clone()];
        l.runs = vec![Run {
            range: 0..11,
            color: color::INSN,
        }];
        b[0] = l;
        compact_block(&mut b);
        assert_eq!(b[0].text, "mov");
        assert_eq!(
            b[0].runs,
            vec![Run {
                range: 0..3,
                color: color::INSN
            }]
        );
    }

    #[test]
    fn compact_block_lets_frame_and_proc_lines_stay_flush_with_the_instructions() {
        // The entry block of a function: `proc`, frame variables, then code
        // that IDA indents. The code must end up at the same column.
        let mut b = vec![
            Line::plain("main    proc near"),
            Line::plain("var_298 = byte ptr -298h"),
            Line::plain("arg_0   = qword ptr  8"),
            insn(8, "sub     rsp, 2B8h"),
            insn(8, "ret"),
        ];
        compact_block(&mut b);
        assert_eq!(b[0].text, "main    proc near");
        assert_eq!(b[1].text, "var_298 = byte ptr -298h");
        assert_eq!(b[3].text, "sub     rsp, 2B8h");
        assert_eq!(b[4].text, "ret");
    }

    #[test]
    fn compact_block_leaves_blocks_without_instructions_alone() {
        let mut b = vec![Line::plain("loc_1:"), Line::plain("; only a comment")];
        compact_block(&mut b);
        assert_eq!(b[0].text, "loc_1:");
        assert_eq!(b[1].text, "; only a comment");
        let mut none: Vec<Line> = Vec::new();
        compact_block(&mut none);
    }
}
