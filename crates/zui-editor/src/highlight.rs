// Pluggable syntax highlighting. A highlighter sees one line at a time plus a
// small state value it can carry to the next line (block comments, triple
// quoted strings). No gpui types.
use crate::doc::LineEdit;
use std::ops::Range;

/// Opaque per-line carry state; `LineState::default()` is "plain code".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineState(pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    Keyword,
    Type,
    Number,
    String,
    Comment,
    Preprocessor,
    Function,
    Builtin,
}

pub trait Highlighter {
    /// Spans are byte ranges into `line`, sorted and non-overlapping. `state`
    /// is the state at the start of the line on entry and at its end on return.
    fn highlight(&self, line: &str, state: &mut LineState) -> Vec<(Range<usize>, HighlightKind)>;
}

pub type Spans = Vec<(Range<usize>, HighlightKind)>;

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

/// Index of `needle` in `hay` at or after `from`.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// End of a quoted literal starting at `start` (the opening quote), honouring
/// backslash escapes; the end of the line if it is unterminated.
fn quoted_end(b: &[u8], start: usize) -> usize {
    let quote = b[start];
    let mut j = start + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// End of a numeric literal starting at `start`. A sign is part of it only
/// right after an exponent marker of a decimal literal.
fn number_end(b: &[u8], start: usize) -> usize {
    let hex = b[start] == b'0' && matches!(b.get(start + 1), Some(b'x' | b'X'));
    let mut j = start;
    while j < b.len() {
        let c = b[j];
        let exp_sign =
            (c == b'+' || c == b'-') && !hex && j > start && matches!(b[j - 1], b'e' | b'E');
        if is_ident(c) || c == b'.' || exp_sign {
            j += 1;
        } else {
            break;
        }
    }
    j
}

fn next_nonspace(b: &[u8], from: usize) -> Option<u8> {
    b.get(from..)?
        .iter()
        .copied()
        .find(|c| *c != b' ' && *c != b'\t')
}

const C_KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "asm",
    "auto",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "const_cast",
    "constexpr",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "dynamic_cast",
    "else",
    "enum",
    "explicit",
    "extern",
    "false",
    "final",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "nullptr",
    "operator",
    "or",
    "override",
    "private",
    "protected",
    "public",
    "register",
    "reinterpret_cast",
    "restrict",
    "return",
    "sizeof",
    "static",
    "static_assert",
    "static_cast",
    "struct",
    "switch",
    "template",
    "this",
    "throw",
    "true",
    "try",
    "typedef",
    "typename",
    "union",
    "using",
    "virtual",
    "volatile",
    "while",
    "NULL",
    "__cdecl",
    "__stdcall",
    "__fastcall",
    "__thiscall",
    "__usercall",
    "__userpurge",
    "__spoils",
    "__noreturn",
    "__declspec",
    "__attribute__",
    "__unaligned",
    "__ptr64",
    "__ptr32",
    "__restrict",
    "__cppobj",
    "__hidden",
    "__shifted",
    "__tag",
    "__struct_ptr",
];

const C_TYPES: &[&str] = &[
    "void",
    "char",
    "short",
    "int",
    "long",
    "float",
    "double",
    "signed",
    "unsigned",
    "bool",
    "_Bool",
    "wchar_t",
    "char8_t",
    "char16_t",
    "char32_t",
    "__int8",
    "__int16",
    "__int32",
    "__int64",
    "__int128",
    "_BYTE",
    "_WORD",
    "_DWORD",
    "_QWORD",
    "_OWORD",
    "_UNKNOWN",
    "BYTE",
    "WORD",
    "DWORD",
    "QWORD",
    "BOOL",
    "BOOLEAN",
    "HANDLE",
    "HMODULE",
    "HINSTANCE",
    "HWND",
    "LPVOID",
    "LPCVOID",
    "LPSTR",
    "LPCSTR",
    "LPWSTR",
    "LPCWSTR",
    "PVOID",
    "LONG",
    "ULONG",
    "UINT",
    "INT",
    "SHORT",
    "USHORT",
    "CHAR",
    "WCHAR",
    "UCHAR",
    "NTSTATUS",
    "HRESULT",
    "va_list",
    "FILE",
    "__m128",
    "__m128i",
    "__m128d",
    "__m256",
    "__m256i",
    "_BOOL1",
    "_BOOL2",
    "_BOOL4",
    "_BOOL8",
    "ea_t",
    "tid_t",
    "uval_t",
    "sval_t",
];

/// C and C++ declarations as IDA prints them (keywords, types, numbers,
/// strings, comments, preprocessor lines, calls).
pub struct CHighlighter;

const C_BLOCK: u8 = 1;

impl Highlighter for CHighlighter {
    fn highlight(&self, line: &str, state: &mut LineState) -> Spans {
        let b = line.as_bytes();
        let mut out: Spans = Vec::new();
        let mut i = 0;
        if state.0 == C_BLOCK {
            match find(b, b"*/", 0) {
                Some(p) => {
                    out.push((0..p + 2, HighlightKind::Comment));
                    i = p + 2;
                    *state = LineState::default();
                }
                None => {
                    if !b.is_empty() {
                        out.push((0..b.len(), HighlightKind::Comment));
                    }
                    return out;
                }
            }
        } else if let Some(first) = b.iter().position(|c| *c != b' ' && *c != b'\t')
            && b[first] == b'#'
        {
            // A directive runs up to a trailing comment.
            let mut j = first;
            while j < b.len() {
                match b[j] {
                    b'"' => j = quoted_end(b, j),
                    b'/' if matches!(b.get(j + 1), Some(b'/' | b'*')) => break,
                    _ => j += 1,
                }
            }
            out.push((first..j, HighlightKind::Preprocessor));
            i = j;
        }

        // After struct/union/enum/class the next identifier names a type.
        let mut expect_tag = false;
        while i < b.len() {
            let c = b[i];
            match c {
                b'/' if b.get(i + 1) == Some(&b'/') => {
                    out.push((i..b.len(), HighlightKind::Comment));
                    break;
                }
                b'/' if b.get(i + 1) == Some(&b'*') => match find(b, b"*/", i + 2) {
                    Some(p) => {
                        out.push((i..p + 2, HighlightKind::Comment));
                        i = p + 2;
                    }
                    None => {
                        out.push((i..b.len(), HighlightKind::Comment));
                        *state = LineState(C_BLOCK);
                        break;
                    }
                },
                b'"' | b'\'' => {
                    let end = quoted_end(b, i);
                    out.push((i..end, HighlightKind::String));
                    expect_tag = false;
                    i = end;
                }
                b'0'..=b'9' if i == 0 || !is_ident(b[i - 1]) => {
                    let end = number_end(b, i);
                    out.push((i..end, HighlightKind::Number));
                    expect_tag = false;
                    i = end;
                }
                _ if is_ident_start(c) => {
                    let mut j = i;
                    while j < b.len() && is_ident(b[j]) {
                        j += 1;
                    }
                    let word = &line[i..j];
                    let kind = if C_KEYWORDS.contains(&word) {
                        if matches!(word, "struct" | "union" | "enum" | "class") {
                            expect_tag = true;
                        }
                        Some(HighlightKind::Keyword)
                    } else if C_TYPES.contains(&word) || (word.len() > 2 && word.ends_with("_t")) {
                        expect_tag = false;
                        Some(HighlightKind::Type)
                    } else if std::mem::take(&mut expect_tag) {
                        Some(HighlightKind::Type)
                    } else if next_nonspace(b, j) == Some(b'(') {
                        Some(HighlightKind::Function)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        out.push((i..j, kind));
                    }
                    i = j;
                }
                b' ' | b'\t' => i += 1,
                _ => {
                    expect_tag = false;
                    i += 1;
                }
            }
        }
        out
    }
}

const PY_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

const PY_BUILTINS: &[&str] = &[
    "abs",
    "all",
    "any",
    "bin",
    "bool",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "dict",
    "dir",
    "enumerate",
    "filter",
    "float",
    "format",
    "getattr",
    "hasattr",
    "hash",
    "hex",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "list",
    "map",
    "max",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
    "Exception",
    "ValueError",
    "TypeError",
    "KeyError",
    "IndexError",
    "RuntimeError",
    "AttributeError",
    "StopIteration",
    "OSError",
    "NotImplementedError",
];

fn is_py_string_prefix(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "r" | "b" | "u" | "f" | "rb" | "br" | "fr" | "rf"
    )
}

/// Python for an IDAPython console (keywords, builtins, strings with prefixes
/// and triple quotes, numbers, comments, decorators, def/class names, calls).
pub struct PythonHighlighter;

const PY_TRIPLE_DQ: u8 = 2;
const PY_TRIPLE_SQ: u8 = 3;

/// End of the triple-quoted literal body starting at `from`, if it closes on
/// this line.
fn triple_close(b: &[u8], from: usize, quote: u8) -> Option<usize> {
    let mut j = from;
    while j < b.len() {
        if b[j] == b'\\' {
            j += 2;
        } else if b[j] == quote && b.get(j + 1) == Some(&quote) && b.get(j + 2) == Some(&quote) {
            return Some(j + 3);
        } else {
            j += 1;
        }
    }
    None
}

fn triple_state(quote: u8) -> LineState {
    LineState(if quote == b'"' {
        PY_TRIPLE_DQ
    } else {
        PY_TRIPLE_SQ
    })
}

/// Pushes the literal whose opening quote is at `quote_at` (the literal,
/// prefix included, starts at `start`). Returns where scanning resumes, or
/// `None` when a triple-quoted literal runs past the end of the line.
fn py_string(b: &[u8], quote_at: usize, start: usize, out: &mut Spans) -> Option<usize> {
    let q = b[quote_at];
    let triple = b.get(quote_at + 1) == Some(&q) && b.get(quote_at + 2) == Some(&q);
    if triple {
        return match triple_close(b, quote_at + 3, q) {
            Some(end) => {
                out.push((start..end, HighlightKind::String));
                Some(end)
            }
            None => {
                out.push((start..b.len(), HighlightKind::String));
                None
            }
        };
    }
    let end = quoted_end(b, quote_at);
    out.push((start..end, HighlightKind::String));
    Some(end)
}

impl Highlighter for PythonHighlighter {
    fn highlight(&self, line: &str, state: &mut LineState) -> Spans {
        let b = line.as_bytes();
        let mut out: Spans = Vec::new();
        let mut i = 0;
        if state.0 != 0 {
            let quote = if state.0 == PY_TRIPLE_DQ { b'"' } else { b'\'' };
            match triple_close(b, 0, quote) {
                Some(end) => {
                    out.push((0..end, HighlightKind::String));
                    i = end;
                    *state = LineState::default();
                }
                None => {
                    if !b.is_empty() {
                        out.push((0..b.len(), HighlightKind::String));
                    }
                    return out;
                }
            }
        }
        let mut expect: Option<HighlightKind> = None;
        let first = b.iter().position(|c| *c != b' ' && *c != b'\t');
        while i < b.len() {
            let c = b[i];
            match c {
                b'#' => {
                    out.push((i..b.len(), HighlightKind::Comment));
                    break;
                }
                b'@' if Some(i) == first => {
                    let mut j = i + 1;
                    while j < b.len() && (is_ident(b[j]) || b[j] == b'.') {
                        j += 1;
                    }
                    out.push((i..j, HighlightKind::Preprocessor));
                    i = j;
                }
                b'"' | b'\'' => match py_string(b, i, i, &mut out) {
                    Some(end) => i = end,
                    None => {
                        *state = triple_state(c);
                        break;
                    }
                },
                b'0'..=b'9' if i == 0 || !is_ident(b[i - 1]) => {
                    let end = number_end(b, i);
                    out.push((i..end, HighlightKind::Number));
                    i = end;
                }
                _ if is_ident_start(c) => {
                    let mut j = i;
                    while j < b.len() && is_ident(b[j]) {
                        j += 1;
                    }
                    let word = &line[i..j];
                    if matches!(b.get(j), Some(b'"' | b'\'')) && is_py_string_prefix(word) {
                        match py_string(b, j, i, &mut out) {
                            Some(end) => i = end,
                            None => {
                                *state = triple_state(b[j]);
                                break;
                            }
                        }
                        continue;
                    }
                    let kind = if PY_KEYWORDS.contains(&word) {
                        expect = match word {
                            "def" => Some(HighlightKind::Function),
                            "class" => Some(HighlightKind::Type),
                            _ => None,
                        };
                        Some(HighlightKind::Keyword)
                    } else if let Some(k) = expect.take() {
                        Some(k)
                    } else if PY_BUILTINS.contains(&word) {
                        Some(HighlightKind::Builtin)
                    } else if next_nonspace(b, j) == Some(b'(') {
                        Some(HighlightKind::Function)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        out.push((i..j, kind));
                    }
                    i = j;
                }
                _ => i += 1,
            }
        }
        out
    }
}

/// Start-of-line states for a whole document, recomputed lazily and only as
/// far as the caller looks. After an edit the old entries below it are kept as
/// hints: once a recomputed state equals its hint, everything further down is
/// known to be unchanged and the scan stops.
pub struct StateCache {
    /// `states[i]` is the state at the start of line `i`.
    states: Vec<LineState>,
    /// `states[..valid]` are correct; entries past it are hints.
    valid: usize,
    /// First index whose hint belongs to unchanged text; `usize::MAX` when
    /// hints cannot be trusted.
    guard: usize,
}

impl Default for StateCache {
    fn default() -> Self {
        Self::new()
    }
}

impl StateCache {
    pub fn new() -> Self {
        Self {
            states: vec![LineState::default()],
            valid: 1,
            guard: usize::MAX,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn apply_edit(&mut self, edit: LineEdit) {
        let first = edit.start_line;
        if first + 1 > self.states.len() {
            return; // never computed that far
        }
        let pending = self.valid < self.states.len();
        self.valid = self.valid.min(first + 1);
        if pending {
            self.states.truncate(self.valid);
            self.guard = usize::MAX;
            return;
        }
        let from = (first + 1).min(self.states.len());
        let to = (first + edit.removed).min(self.states.len());
        let fill = std::iter::repeat_n(LineState::default(), edit.inserted.saturating_sub(1));
        self.states.splice(from..to, fill);
        self.guard = first + edit.inserted;
    }

    pub fn start_state(
        &mut self,
        hl: &dyn Highlighter,
        lines: &[String],
        line: usize,
    ) -> LineState {
        let line = line.min(lines.len());
        while self.valid <= line {
            let i = self.valid;
            let mut st = self.states[i - 1];
            hl.highlight(&lines[i - 1], &mut st);
            if i < self.states.len() {
                if i >= self.guard && self.states[i] == st {
                    self.valid = self.states.len();
                    break;
                }
                self.states[i] = st;
            } else {
                self.states.push(st);
            }
            self.valid += 1;
        }
        self.states[line.min(self.states.len() - 1)]
    }
}

#[cfg(test)]
mod tests;
