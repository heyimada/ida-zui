use super::*;

fn split(s: &str) -> Option<(String, String)> {
    split_field(s)
}

fn t(a: &str, b: &str) -> Option<(String, String)> {
    Some((a.to_owned(), b.to_owned()))
}

#[test]
fn plain_fields_split_into_type_and_name() {
    assert_eq!(split("int a"), t("int", "a"));
    assert_eq!(split("unsigned int flags;"), t("unsigned int", "flags"));
    assert_eq!(split("  int   a ; "), t("int", "a"));
    assert_eq!(split("struct Foo f"), t("struct Foo", "f"));
}

#[test]
fn pointers_keep_the_star_with_the_type() {
    assert_eq!(split("void *next"), t("void *", "next"));
    assert_eq!(split("const char *p"), t("const char *", "p"));
    assert_eq!(split("char **pp"), t("char **", "pp"));
}

#[test]
fn arrays_move_the_dimensions_to_the_type() {
    assert_eq!(split("char name[16]"), t("char [16]", "name"));
    assert_eq!(split("char *names[4]"), t("char * [4]", "names"));
    assert_eq!(
        split("unsigned char buf[2][3]"),
        t("unsigned char [2][3]", "buf")
    );
}

#[test]
fn function_pointers_lose_only_their_name() {
    assert_eq!(split("int (*cb)(int, char)"), t("int (*)(int, char)", "cb"));
    assert_eq!(
        split("void (__fastcall *fn)(void *)"),
        t("void (__fastcall *)(void *)", "fn")
    );
}

#[test]
fn text_without_a_name_is_rejected() {
    assert_eq!(split(""), None);
    assert_eq!(split("   "), None);
    assert_eq!(split("int"), None);
    assert_eq!(split("int *"), None);
    assert_eq!(split("char [16]"), None);
}

#[test]
fn names_must_be_identifiers() {
    assert_eq!(split("int 9a"), None);
    assert_eq!(split("int a-b"), None);
    assert_eq!(split("int _ok1"), t("int", "_ok1"));
}

#[test]
fn values_parse_like_ida_accepts_them() {
    assert_eq!(parse_value("32"), Some(32));
    assert_eq!(parse_value("0x20"), Some(32));
    assert_eq!(parse_value("0X1f"), Some(31));
    assert_eq!(parse_value("0b101"), Some(5));
    assert_eq!(parse_value(" 7 "), Some(7));
    assert_eq!(parse_value("-1"), Some(u64::MAX));
    assert_eq!(parse_value("-0x10"), Some(0u64.wrapping_sub(16)));
    assert_eq!(parse_value("1<<4"), Some(16));
    assert_eq!(parse_value("1 << 31"), Some(0x8000_0000));
}

#[test]
fn bad_values_are_rejected() {
    assert_eq!(parse_value(""), None);
    assert_eq!(parse_value("0x"), None);
    assert_eq!(parse_value("12abc"), None);
    assert_eq!(parse_value("1<<64"), None);
    assert_eq!(parse_value("0x1ffffffffffffffff"), None);
    assert_eq!(parse_value("--1"), None);
}

#[test]
fn values_format_in_either_radix() {
    assert_eq!(format_value(32, true), "0x20");
    assert_eq!(format_value(255, true), "0xFF");
    assert_eq!(format_value(32, false), "32");
    assert_eq!(format_value(0xFFFF_FFFF, false), "4294967295");
    assert_eq!(format_value(0, true), "0x0");
}
