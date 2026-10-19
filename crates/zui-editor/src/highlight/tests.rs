// Failure modes covered: keywords inside longer identifiers, numbers glued to
// identifiers, comment/string markers inside each other, block comments and
// triple-quoted strings carried across lines and left unterminated, escaped
// quotes, preprocessor lines with trailing comments, multi-byte text inside
// spans, the state cache after edits that change line count or state, and the
// cache not rescanning the document when an edit leaves the state unchanged.
use super::*;
use HighlightKind::{Builtin, Comment, Function, Keyword, Number, Preprocessor, Type};
use std::cell::Cell;
const STR: HighlightKind = HighlightKind::String;

/// Highlights `line` and returns (text, kind) pairs plus the end state.
fn run(
    hl: &dyn Highlighter,
    line: &str,
    start: LineState,
) -> (Vec<(String, HighlightKind)>, LineState) {
    let mut st = start;
    let spans = hl.highlight(line, &mut st);
    let mut last = 0;
    for (r, _) in &spans {
        assert!(
            r.start >= last && r.start < r.end && r.end <= line.len(),
            "bad span {r:?} in {line:?}"
        );
        assert!(line.is_char_boundary(r.start) && line.is_char_boundary(r.end));
        last = r.end;
    }
    (
        spans
            .into_iter()
            .map(|(r, k)| (line[r].to_string(), k))
            .collect(),
        st,
    )
}

fn c(line: &str) -> Vec<(String, HighlightKind)> {
    run(&CHighlighter, line, LineState::default()).0
}

fn py(line: &str) -> Vec<(String, HighlightKind)> {
    run(&PythonHighlighter, line, LineState::default()).0
}

fn t(s: &str, k: HighlightKind) -> (String, HighlightKind) {
    (s.to_string(), k)
}

#[test]
fn c_keywords_types_and_numbers() {
    assert_eq!(
        c("unsigned int x = 0x1F;"),
        vec![t("unsigned", Type), t("int", Type), t("0x1F", Number)]
    );
    assert_eq!(
        c("return 3.14e-2f;"),
        vec![t("return", Keyword), t("3.14e-2f", Number)]
    );
    assert_eq!(
        c("x = 12u + 0b101;"),
        vec![t("12u", Number), t("0b101", Number)]
    );
}

#[test]
fn c_names_that_contain_keywords_or_digits_are_not_highlighted() {
    assert_eq!(c("int interface_x = intx1;"), vec![t("int", Type)]);
    assert_eq!(c("x1 = y2;"), vec![]);
}

#[test]
fn c_tag_names_and_ida_typedefs_are_types() {
    assert_eq!(
        c("struct Foo {"),
        vec![t("struct", Keyword), t("Foo", Type)]
    );
    assert_eq!(
        c("enum class E : int"),
        vec![
            t("enum", Keyword),
            t("class", Keyword),
            t("E", Type),
            t("int", Type)
        ]
    );
    assert_eq!(
        c("_DWORD a; __int64 b; QWORD c;"),
        vec![t("_DWORD", Type), t("__int64", Type), t("QWORD", Type)]
    );
    assert_eq!(c("uint8_t *p;"), vec![t("uint8_t", Type)]);
}

#[test]
fn c_calls_are_functions_but_keywords_are_not() {
    assert_eq!(c("foo(1)"), vec![t("foo", Function), t("1", Number)]);
    assert_eq!(
        c("if (x) while(y)"),
        vec![t("if", Keyword), t("while", Keyword)]
    );
    assert_eq!(c("sizeof(int)"), vec![t("sizeof", Keyword), t("int", Type)]);
    assert_eq!(
        c("int main(void)"),
        vec![t("int", Type), t("main", Function), t("void", Type)]
    );
}

#[test]
fn c_strings_and_chars_honour_escapes_and_stay_on_their_line() {
    assert_eq!(
        c(r#"char *s = "a\"b";"#),
        vec![t("char", Type), t(r#""a\"b""#, STR)]
    );
    assert_eq!(c(r"c = '\'';"), vec![t(r"'\''", STR)]);
    let (spans, st) = run(&CHighlighter, "\"unterminated", LineState::default());
    assert_eq!(spans, vec![t("\"unterminated", STR)]);
    assert_eq!(st, LineState::default());
}

#[test]
fn c_comment_markers_inside_strings_are_ignored_and_vice_versa() {
    let (spans, st) = run(&CHighlighter, "\"/* no */\" int", LineState::default());
    assert_eq!(spans, vec![t("\"/* no */\"", STR), t("int", Type)]);
    assert_eq!(st, LineState::default());
    assert_eq!(
        c("/* \"q */ int"),
        vec![t("/* \"q */", Comment), t("int", Type)]
    );
    assert_eq!(
        c("int x; // \"not a string\""),
        vec![t("int", Type), t("// \"not a string\"", Comment)]
    );
}

#[test]
fn c_block_comment_carries_across_lines() {
    let (a, s1) = run(&CHighlighter, "int a; /* start", LineState::default());
    assert_eq!(a, vec![t("int", Type), t("/* start", Comment)]);
    assert_ne!(s1, LineState::default());
    let (b, s2) = run(&CHighlighter, "  middle int", s1);
    assert_eq!(b, vec![t("  middle int", Comment)]);
    assert_eq!(s2, s1);
    let (d, s3) = run(&CHighlighter, "end */ int b;", s2);
    assert_eq!(d, vec![t("end */", Comment), t("int", Type)]);
    assert_eq!(s3, LineState::default());
}

#[test]
fn c_second_comment_can_open_on_the_closing_line() {
    let open = run(&CHighlighter, "/*", LineState::default()).1;
    let (spans, st) = run(&CHighlighter, "*/ x /* y", open);
    assert_eq!(spans, vec![t("*/", Comment), t("/* y", Comment)]);
    assert_eq!(st, open);
}

#[test]
fn c_unterminated_comment_keeps_its_state_to_the_end() {
    let mut st = LineState::default();
    for line in ["/* never closed", "int still_comment;", ""] {
        CHighlighter.highlight(line, &mut st);
    }
    assert_ne!(st, LineState::default());
}

#[test]
fn c_preprocessor_lines() {
    assert_eq!(
        c("#include <stdio.h>"),
        vec![t("#include <stdio.h>", Preprocessor)]
    );
    assert_eq!(
        c("  #pragma pack(push, 1)"),
        vec![t("#pragma pack(push, 1)", Preprocessor)]
    );
    assert_eq!(
        c("#define X 1 // c"),
        vec![t("#define X 1 ", Preprocessor), t("// c", Comment)]
    );
    assert_eq!(c("a = b # c"), vec![]);
}

#[test]
fn c_spans_stay_on_char_boundaries_with_multibyte_text() {
    assert_eq!(
        c("\"日本\" int /* é */"),
        vec![t("\"日本\"", STR), t("int", Type), t("/* é */", Comment)]
    );
}

#[test]
fn python_keywords_defs_and_classes() {
    assert_eq!(
        py("def foo(x):"),
        vec![t("def", Keyword), t("foo", Function)]
    );
    assert_eq!(
        py("class Bar(Base):"),
        vec![t("class", Keyword), t("Bar", Type)]
    );
    assert_eq!(py("import idc"), vec![t("import", Keyword)]);
    assert_eq!(
        py("x = None if iffy else True"),
        vec![
            t("None", Keyword),
            t("if", Keyword),
            t("else", Keyword),
            t("True", Keyword)
        ]
    );
}

#[test]
fn python_builtins_and_method_calls() {
    assert_eq!(
        py("print(len(x))"),
        vec![t("print", Builtin), t("len", Builtin)]
    );
    assert_eq!(py("idc.get_name(ea)"), vec![t("get_name", Function)]);
    assert_eq!(py("self.x"), vec![]);
}

#[test]
fn python_numbers() {
    assert_eq!(
        py("x = 0x1f + 1_000 + 3.14 + 1e5 + y2"),
        vec![
            t("0x1f", Number),
            t("1_000", Number),
            t("3.14", Number),
            t("1e5", Number)
        ]
    );
}

#[test]
fn python_hash_inside_a_string_is_not_a_comment() {
    assert_eq!(
        py("name = \"a#b\"  # real"),
        vec![t("\"a#b\"", STR), t("# real", Comment)]
    );
}

#[test]
fn python_string_prefixes_and_escapes() {
    assert_eq!(py("f\"hi {x}\""), vec![t("f\"hi {x}\"", STR)]);
    assert_eq!(py("rb'\\x00'"), vec![t("rb'\\x00'", STR)]);
    assert_eq!(py(r"'it\'s'"), vec![t(r"'it\'s'", STR)]);
}

#[test]
fn python_triple_quoted_strings_carry_across_lines() {
    let (a, s1) = run(&PythonHighlighter, "x = \"\"\"doc", LineState::default());
    assert_eq!(a, vec![t("\"\"\"doc", STR)]);
    assert_ne!(s1, LineState::default());
    let (b, s2) = run(&PythonHighlighter, "# not a comment 'x'", s1);
    assert_eq!(b, vec![t("# not a comment 'x'", STR)]);
    assert_eq!(s2, s1);
    let (d, s3) = run(&PythonHighlighter, "end\"\"\" + y", s2);
    assert_eq!(d, vec![t("end\"\"\"", STR)]);
    assert_eq!(s3, LineState::default());
    // A different quote style does not close it.
    let (_, other) = run(&PythonHighlighter, "'''", LineState::default());
    assert_ne!(other, s1);
    assert_eq!(
        py("\"\"\"abc\"\"\" # c"),
        vec![t("\"\"\"abc\"\"\"", STR), t("# c", Comment)]
    );
}

#[test]
fn python_decorators_are_directive_like() {
    assert_eq!(py("@staticmethod"), vec![t("@staticmethod", Preprocessor)]);
}

#[test]
fn python_spans_stay_on_char_boundaries_with_multibyte_text() {
    assert_eq!(
        py("s = 'é日' # ü"),
        vec![t("'é日'", STR), t("# ü", Comment)]
    );
}

// ---- StateCache ----

struct Counting<'a> {
    inner: &'a dyn Highlighter,
    calls: Cell<usize>,
}

impl Highlighter for Counting<'_> {
    fn highlight(&self, line: &str, state: &mut LineState) -> Vec<(Range<usize>, HighlightKind)> {
        self.calls.set(self.calls.get() + 1);
        self.inner.highlight(line, state)
    }
}

fn lines(src: &str) -> Vec<String> {
    src.split('\n').map(String::from).collect()
}

fn fresh_states(hl: &dyn Highlighter, lines: &[String]) -> Vec<LineState> {
    let mut st = LineState::default();
    lines
        .iter()
        .map(|l| {
            let at = st;
            hl.highlight(l, &mut st);
            at
        })
        .collect()
}

fn assert_cache_matches_fresh(cache: &mut StateCache, hl: &dyn Highlighter, lines: &[String]) {
    let want = fresh_states(hl, lines);
    for i in (0..lines.len()).rev() {
        assert_eq!(cache.start_state(hl, lines, i), want[i], "line {i}");
    }
}

#[test]
fn cache_tracks_a_comment_opened_and_closed_by_edits() {
    let mut doc = lines("int a;\nint b;\nint c;\nint d;");
    let mut cache = StateCache::new();
    assert_cache_matches_fresh(&mut cache, &CHighlighter, &doc);

    doc[1] = "int b; /*".into();
    cache.apply_edit(LineEdit {
        start_line: 1,
        removed: 1,
        inserted: 1,
    });
    assert_cache_matches_fresh(&mut cache, &CHighlighter, &doc);
    assert_ne!(
        cache.start_state(&CHighlighter, &doc, 3),
        LineState::default()
    );

    doc[2] = "*/ int c;".into();
    cache.apply_edit(LineEdit {
        start_line: 2,
        removed: 1,
        inserted: 1,
    });
    assert_cache_matches_fresh(&mut cache, &CHighlighter, &doc);
    assert_eq!(
        cache.start_state(&CHighlighter, &doc, 3),
        LineState::default()
    );
}

#[test]
fn cache_follows_inserted_and_removed_lines() {
    let mut doc = lines("a\n/* open\nb\nc */\nd");
    let mut cache = StateCache::new();
    assert_cache_matches_fresh(&mut cache, &CHighlighter, &doc);

    doc.splice(0..1, ["x".to_string(), "y".to_string(), "z".to_string()]);
    cache.apply_edit(LineEdit {
        start_line: 0,
        removed: 1,
        inserted: 3,
    });
    assert_cache_matches_fresh(&mut cache, &CHighlighter, &doc);

    doc.splice(1..4, ["w".to_string()]);
    cache.apply_edit(LineEdit {
        start_line: 1,
        removed: 3,
        inserted: 1,
    });
    assert_cache_matches_fresh(&mut cache, &CHighlighter, &doc);
}

#[test]
fn cache_does_not_rescan_the_document_when_an_edit_leaves_the_state_unchanged() {
    let mut doc: Vec<String> = (0..1000).map(|i| format!("int v{i};")).collect();
    let counting = Counting {
        inner: &CHighlighter,
        calls: Cell::new(0),
    };
    let mut cache = StateCache::new();
    cache.start_state(&counting, &doc, 900);
    assert!(counting.calls.get() >= 900);

    counting.calls.set(0);
    doc[10] = "int changed;".into();
    cache.apply_edit(LineEdit {
        start_line: 10,
        removed: 1,
        inserted: 1,
    });
    assert_eq!(
        cache.start_state(&counting, &doc, 900),
        LineState::default()
    );
    assert!(
        counting.calls.get() < 10,
        "rescanned {} lines",
        counting.calls.get()
    );
}

#[test]
fn cache_reset_forgets_everything() {
    let mut doc = lines("/* a\nb");
    let mut cache = StateCache::new();
    assert_ne!(
        cache.start_state(&CHighlighter, &doc, 1),
        LineState::default()
    );
    doc = lines("x\ny");
    cache.reset();
    assert_eq!(
        cache.start_state(&CHighlighter, &doc, 1),
        LineState::default()
    );
}
