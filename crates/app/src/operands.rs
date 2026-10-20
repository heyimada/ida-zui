//! Pure helpers behind Edit > Operand, Arrays and Patch program: which operand
//! the caret is on, which representations to check, and the checks the dialogs
//! make before they ask the host. No UI, no host.

use ida_proto::{ArrayElem, OpRepr, OpReprKind, OpReprRow};

/// The operand under the caret and how every operand of its instruction is
/// shown, for the check marks of Edit > Operand.
#[derive(Clone, Debug, Default)]
pub struct Ops {
    pub rows: Vec<OpReprRow>,
    pub op: u32,
    /// The caret is on an operand of the line (not on a label or a comment).
    pub on_operand: bool,
}

impl Ops {
    pub fn kind(&self) -> OpReprKind {
        kind_of(&self.rows, self.op)
    }

    /// Whether the caret is on an operand the representation commands apply to.
    pub fn has_operand(&self) -> bool {
        self.on_operand && self.rows.iter().any(|r| r.op == self.op)
    }

    pub fn negated(&self) -> bool {
        is_negated(&self.rows, self.op)
    }

    pub fn inverted(&self) -> bool {
        is_inverted(&self.rows, self.op)
    }
}
use std::path::{Path, PathBuf};

/// The operand under the caret of a listing line: none on a line that has no
/// operands (a label, a `proc near` header, a comment, a blank).
pub fn caret_operand(line: &ida_proto::tags::Line, col: usize) -> Option<u32> {
    if line.operands.is_empty() {
        return None;
    }
    operand_at(&line.text, col)
}

/// The 0-based operand under byte column `col` of a listing line: the caret
/// on the mnemonic (or before it) is operand 0, each comma starts the next
/// operand, and a comment (`;`) has none. Commas inside brackets, parentheses
/// and quotes do not split.
pub fn operand_at(text: &str, col: usize) -> Option<u32> {
    let comment = comment_start(text).unwrap_or(text.len());
    if col >= comment && comment < text.len() {
        return None;
    }
    let code = &text[..comment];
    let first = code.find(|c: char| !c.is_whitespace())?;
    let mnemonic_end = code[first..]
        .find(char::is_whitespace)
        .map_or(code.len(), |i| first + i);
    if col <= mnemonic_end {
        return Some(0);
    }
    let (mut depth, mut quote, mut op) = (0i32, None::<char>, 0u32);
    for (i, c) in code.char_indices().filter(|(i, _)| *i >= mnemonic_end) {
        if i >= col {
            break;
        }
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '(') => depth += 1,
            (None, ']' | ')') => depth -= 1,
            (None, ',') if depth <= 0 => op += 1,
            _ => {}
        }
    }
    Some(op)
}

/// Where a `;` comment starts, ignoring semicolons inside quotes.
fn comment_start(text: &str) -> Option<usize> {
    let mut quote = None::<char>;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, ';') => return Some(i),
            _ => {}
        }
    }
    None
}

/// IDA's `H`: hexadecimal, unless the operand already is, then decimal.
pub fn hex_or_decimal(rows: &[OpReprRow], op: u32) -> OpRepr {
    match kind_of(rows, op) {
        OpReprKind::Hex => OpRepr::Decimal,
        _ => OpRepr::Hex,
    }
}

/// How operand `op` is shown now (`Default` when the host reported nothing).
pub fn kind_of(rows: &[OpReprRow], op: u32) -> OpReprKind {
    rows.iter()
        .find(|r| r.op == op)
        .map_or(OpReprKind::Default, |r| r.kind)
}

pub fn is_negated(rows: &[OpReprRow], op: u32) -> bool {
    rows.iter().any(|r| r.op == op && r.negated)
}

pub fn is_inverted(rows: &[OpReprRow], op: u32) -> bool {
    rows.iter().any(|r| r.op == op && r.inverted)
}

/// Size in bytes of one array element, when it is fixed.
pub fn elem_size(elem: &ArrayElem) -> Option<u32> {
    Some(match elem {
        ArrayElem::Byte => 1,
        ArrayElem::Word => 2,
        ArrayElem::Dword | ArrayElem::Float => 4,
        ArrayElem::Qword | ArrayElem::Double => 8,
        ArrayElem::Oword => 16,
        ArrayElem::Struct { .. } => return None,
    })
}

/// An address as the dialogs show it: hex, with or without `0x` or a trailing `h`.
pub fn parse_address(text: &str) -> Option<u64> {
    let t = text.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    let t = t.strip_suffix(['h', 'H']).unwrap_or(t);
    u64::from_str_radix(t, 16).ok()
}

/// The array dialog's count field: a positive number.
pub fn parse_count(text: &str) -> Result<u32, String> {
    let n: u32 = text
        .trim()
        .parse()
        .map_err(|_| format!("'{}' is not a number", text.trim()))?;
    if n == 0 {
        return Err("The array needs at least one element".into());
    }
    Ok(n)
}

/// What the array would overwrite when `count` elements of `elem_bytes` do
/// not fit in the `max_count` elements of room before the next item (0 when
/// the room is not known). Making the array undefines those items.
pub fn overwrite_note(elem_bytes: u32, count: u32, max_count: u32) -> Option<String> {
    if max_count == 0 || count <= max_count {
        return None;
    }
    let bytes = u64::from(elem_bytes) * u64::from(count - max_count);
    Some(format!(
        "Overwrites the {bytes} byte{} of the items that follow",
        if bytes == 1 { "" } else { "s" }
    ))
}

/// The preview line under the array fields.
pub fn array_preview(elem: &ArrayElem, size: Option<u32>, count: u32) -> String {
    match size.or_else(|| elem_size(elem)) {
        Some(size) => format!(
            "{count} element{} of {size} byte{} = {} bytes",
            if count == 1 { "" } else { "s" },
            if size == 1 { "" } else { "s" },
            u64::from(count) * u64::from(size)
        ),
        None => format!("{count} elements of the structure"),
    }
}

/// Hex bytes as people type them (`48 8B`, `0x48,0x8b`, `488b`) as plain pairs
/// (`48 8B`), or what is wrong with them.
pub fn normalize_hex(text: &str) -> Result<String, String> {
    let cleaned: String = text
        .replace("0x", "")
        .replace("0X", "")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    if cleaned.is_empty() {
        return Err("Enter the bytes as hex pairs, e.g. 90 90 CC".into());
    }
    if let Some(bad) = cleaned.chars().find(|c| !c.is_ascii_hexdigit()) {
        return Err(format!("'{bad}' is not a hex digit"));
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err("A byte needs two hex digits".into());
    }
    let pairs: Vec<String> = cleaned
        .as_bytes()
        .chunks(2)
        .map(|p| String::from_utf8_lossy(p).to_uppercase())
        .collect();
    Ok(pairs.join(" "))
}

/// Where "Apply patches to input file" writes by default: the input's name
/// plus `.patched`, in the same folder. Never the input itself.
pub fn patched_path(input: &Path) -> PathBuf {
    let mut name = input.file_name().unwrap_or_default().to_os_string();
    name.push(".patched");
    input.with_file_name(name)
}

/// The output path must not be the input file (IDA would patch it in place).
pub fn output_problem(input: &Path, output: &Path) -> Option<&'static str> {
    if output.as_os_str().is_empty() {
        Some("Choose where to write the patched copy")
    } else if same_file(input, output) {
        Some("The patched copy cannot overwrite the input file")
    } else {
        None
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_caret_on_the_mnemonic_or_the_first_operand_is_operand_zero() {
        let line = "    mov     rax, [rbp+var_8]";
        assert_eq!(operand_at(line, 0), Some(0), "in the indent");
        assert_eq!(operand_at(line, 6), Some(0), "on the mnemonic");
        assert_eq!(operand_at(line, 13), Some(0), "on rax");
    }

    #[test]
    fn each_comma_starts_the_next_operand() {
        let line = "    imul    eax, ecx, 28h";
        assert_eq!(operand_at(line, 13), Some(0));
        assert_eq!(operand_at(line, 18), Some(1));
        assert_eq!(operand_at(line, line.len() - 1), Some(2));
        assert_eq!(
            operand_at(line, line.len()),
            Some(2),
            "past the end: the last"
        );
    }

    #[test]
    fn commas_inside_brackets_and_quotes_do_not_split() {
        let line = "    lea     rcx, [rax+rbx*2], 'a,b'";
        assert_eq!(operand_at(line, 21), Some(1), "inside the bracket");
        assert_eq!(operand_at(line, 32), Some(2), "inside the quotes");
        let data = "    db 'x;y', 0";
        assert_eq!(
            operand_at(data, data.len() - 1),
            Some(1),
            "; in quotes is no comment"
        );
    }

    #[test]
    fn a_comment_has_no_operand() {
        let line = "    mov     eax, 1 ; set, up";
        assert_eq!(operand_at(line, 18), Some(1));
        assert_eq!(operand_at(line, 25), None);
        assert_eq!(operand_at("; only a comment", 3), None);
        assert_eq!(operand_at("", 0), None);
    }

    fn row(op: u32, kind: OpReprKind) -> OpReprRow {
        OpReprRow {
            op,
            kind,
            negated: false,
            inverted: false,
            detail: String::new(),
        }
    }

    #[test]
    fn h_toggles_between_hex_and_decimal_per_operand() {
        let rows = [row(0, OpReprKind::Hex), row(1, OpReprKind::Default)];
        assert_eq!(hex_or_decimal(&rows, 0), OpRepr::Decimal);
        assert_eq!(hex_or_decimal(&rows, 1), OpRepr::Hex);
        assert_eq!(
            hex_or_decimal(&rows, 5),
            OpRepr::Hex,
            "an operand nobody reported"
        );
        let dec = [row(0, OpReprKind::Decimal)];
        assert_eq!(hex_or_decimal(&dec, 0), OpRepr::Hex);
    }

    #[test]
    fn check_marks_follow_the_operand_state() {
        let rows = [OpReprRow {
            negated: true,
            ..row(0, OpReprKind::Char)
        }];
        assert_eq!(kind_of(&rows, 0), OpReprKind::Char);
        assert_eq!(kind_of(&rows, 1), OpReprKind::Default);
        assert!(is_negated(&rows, 0) && !is_negated(&rows, 1));
        assert!(!is_inverted(&rows, 0));
    }

    #[test]
    fn an_address_field_is_hex_unless_it_says_otherwise() {
        assert_eq!(parse_address("140002281"), Some(0x1_4000_2281));
        assert_eq!(parse_address(" 0x1400018C0 "), Some(0x1_4000_18C0));
        assert_eq!(parse_address("1400018c0h"), Some(0x1_4000_18C0));
        assert_eq!(parse_address(""), None);
        assert_eq!(parse_address("0x"), None);
        assert_eq!(parse_address("main"), None);
    }

    #[test]
    fn the_array_count_must_be_a_positive_number() {
        assert_eq!(parse_count(" 16 "), Ok(16));
        assert_eq!(parse_count("1000"), Ok(1000));
        assert!(parse_count("0").is_err());
        assert!(parse_count("-1").is_err());
        assert!(parse_count("ten").unwrap_err().contains("ten"));
        assert!(parse_count("").is_err());
    }

    #[test]
    fn an_array_longer_than_the_room_says_how_much_it_overwrites() {
        // 4 dwords = 16 bytes where 1 element (4 bytes) fits before the next item.
        assert_eq!(
            overwrite_note(4, 4, 1).as_deref(),
            Some("Overwrites the 12 bytes of the items that follow")
        );
        assert_eq!(overwrite_note(4, 1, 1), None, "it fits");
        assert_eq!(overwrite_note(4, 9, 0), None, "no limit known");
        assert_eq!(
            overwrite_note(1, 3, 2).as_deref(),
            Some("Overwrites the 1 byte of the items that follow")
        );
    }

    #[test]
    fn the_preview_multiplies_count_by_element_size() {
        assert_eq!(
            array_preview(&ArrayElem::Dword, None, 10),
            "10 elements of 4 bytes = 40 bytes"
        );
        assert_eq!(
            array_preview(&ArrayElem::Byte, None, 1),
            "1 element of 1 byte = 1 bytes"
        );
        let s = ArrayElem::Struct { name: "S".into() };
        assert_eq!(
            array_preview(&s, Some(24), 2),
            "2 elements of 24 bytes = 48 bytes"
        );
        assert_eq!(array_preview(&s, None, 2), "2 elements of the structure");
    }

    #[test]
    fn hex_input_is_normalised_or_explained() {
        assert_eq!(normalize_hex("90 90 cc").as_deref(), Ok("90 90 CC"));
        assert_eq!(normalize_hex("0x48,0x8b").as_deref(), Ok("48 8B"));
        assert_eq!(normalize_hex("488B05").as_deref(), Ok("48 8B 05"));
        assert!(normalize_hex("").unwrap_err().contains("hex pairs"));
        assert!(normalize_hex("9").unwrap_err().contains("two hex digits"));
        assert!(normalize_hex("9G").unwrap_err().contains('G'));
    }

    #[test]
    fn the_patched_copy_sits_beside_the_input_and_never_replaces_it() {
        let input = Path::new("C:/work/find.exe");
        let out = patched_path(input);
        assert_eq!(out, Path::new("C:/work/find.exe.patched"));
        assert_eq!(output_problem(input, &out), None);
        assert!(output_problem(input, input).is_some());
        assert!(output_problem(input, Path::new("")).is_some());
    }
}
