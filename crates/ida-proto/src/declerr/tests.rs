use super::*;

fn lines(decl: &str, messages: &str) -> Vec<(usize, String)> {
    locate(decl, messages)
        .into_iter()
        .map(|e| (e.line, e.message))
        .collect()
}

// The messages below are what IDA's parse_decls printed for these inputs.

#[test]
fn syntax_error_points_at_its_token() {
    let decl = "struct A {\n  int a;\n  foo_t b;\n};";
    let got = lines(decl, "Syntax error near: foo_t\nSyntax error near: }\n");
    assert_eq!(got[0], (2, "Syntax error near: foo_t".to_owned()));
    // The closing brace is searched after the previous error, not from the top.
    assert_eq!(got[1].0, 3);
}

#[test]
fn a_token_seen_earlier_does_not_steal_the_error() {
    let decl = "int x;\nint y;\nstruct S { int q\n};";
    // `int` occurs on every line; the error is about the `}` after `q`.
    let got = lines(decl, "Syntax error near: }\n");
    assert_eq!(got, vec![(3, "Syntax error near: }".to_owned())]);
}

#[test]
fn errors_after_a_good_statement_land_in_the_later_statement() {
    let decl = "struct A6 { int a; };\nstruct A6b { unknown b; };\nstruct A7 { int z; };";
    let got = lines(decl, "Syntax error near: unknown\nSyntax error near: }\n");
    assert_eq!(got.iter().map(|e| e.0).collect::<Vec<_>>(), vec![1, 1]);
}

#[test]
fn end_of_input_is_the_last_line_with_text() {
    let decl = "struct A4 {\n  int a;\n\n";
    assert_eq!(
        lines(decl, "Syntax error near: <END>\n"),
        vec![(1, "Syntax error near: <END>".to_owned())]
    );
}

#[test]
fn duplicate_names_point_at_the_second_definition() {
    let decl = "struct A2 {\n  int a;\n  int a;\n};";
    let got = lines(
        decl,
        "Field 'a' has already been defined\nSyntax error near: }\n",
    );
    assert_eq!(got[0].0, 2);
}

#[test]
fn quoted_names_match_whole_words_only() {
    let decl = "struct S {\n  int data;\n  int a;\n  int a;\n};";
    let got = lines(decl, "Field 'a' has already been defined\n");
    assert_eq!(got[0].0, 3);
}

#[test]
fn undefined_type_name_is_found_by_its_quoted_name() {
    let decl = "struct A11 {\n  A11 inner;\n};";
    let got = lines(decl, "Undefined type name 'A11'\nSyntax error near: }\n");
    // Line 0 declares the struct; the offending use is on line 1.
    assert_eq!(got[0].0, 1);
}

#[test]
fn comments_are_not_searched() {
    let decl = "// badtype is explained here\nstruct S {\n  badtype x;\n};";
    let got = lines(decl, "Syntax error near: badtype\n");
    assert_eq!(got[0].0, 2);
}

#[test]
fn messages_without_a_location_use_the_position_so_far() {
    let decl = "struct A3 {\n  int a[-1];\n};";
    let got = lines(
        decl,
        "Negative sized arrays are forbidden\nSyntax error near: }\n",
    );
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].1, "Negative sized arrays are forbidden");
    assert_eq!(got[1].0, 2);
}

#[test]
fn a_token_that_is_not_in_the_text_falls_back_to_the_cursor() {
    let got = lines("int a;\nint b;", "Syntax error near: nothere\n");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, 0);
}

#[test]
fn no_messages_no_errors() {
    assert!(locate("struct S { int a; };", "").is_empty());
    assert!(locate("", "\n\n").is_empty());
}

#[test]
fn crlf_text_counts_lines_the_same() {
    let decl = "struct S {\r\n  int a;\r\n  bad b;\r\n};";
    assert_eq!(lines(decl, "Syntax error near: bad\n")[0].0, 2);
}

#[test]
fn a_missing_semicolon_is_found_past_good_statements() {
    // The first `}` closes a valid struct; the error is the one after `int a`.
    let decl = "struct E2eOk {\n  int a;\n};\nstruct E2eBad2 {\n  int a\n};";
    let got = lines(decl, "Syntax error near: }\n");
    assert_eq!(got[0].0, 5);
}

#[test]
fn a_brace_after_a_semicolon_still_falls_back_to_the_first_one() {
    let decl = "struct A3 {\n  int a[-1];\n};";
    assert_eq!(lines(decl, "Syntax error near: }\n")[0].0, 2);
}
