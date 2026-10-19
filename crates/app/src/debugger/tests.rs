use super::*;

fn reg(name: &str, hex: &str) -> DbgReg {
    DbgReg {
        name: name.into(),
        bits: (hex.len() * 4) as u32,
        class: String::new(),
        hex: hex.into(),
        float: false,
        ip: false,
        sp: false,
        fp: false,
        address: false,
        readonly: false,
        bit_names: Vec::new(),
    }
}

#[test]
fn shifting_an_address_follows_a_rebase_in_either_direction() {
    assert_eq!(shift(0x1000, 0x40_0000), 0x40_1000);
    assert_eq!(shift(0x40_1000, -0x40_0000), 0x1000);
    // A shift never wraps below zero into a huge address by accident.
    assert_eq!(shift(0x10, -0x20), 0);
}

#[test]
fn no_register_counts_as_changed_without_a_previous_stop() {
    let regs = vec![reg("RAX", "0000000000000001")];
    assert!(changed_regs(&HashMap::new(), &regs).is_empty());
}

#[test]
fn only_registers_with_a_different_value_are_changed() {
    let prev: HashMap<String, String> = [
        ("RAX".to_owned(), "0000000000000001".to_owned()),
        ("RBX".to_owned(), "0000000000000002".to_owned()),
    ]
    .into();
    let regs = vec![
        reg("RAX", "0000000000000001"),
        reg("RBX", "0000000000000009"),
        // Not in the previous snapshot (an older stop had no such register).
        reg("RCX", "0000000000000003"),
    ];
    let c = changed_regs(&prev, &regs);
    assert!(c.contains("RBX"));
    assert!(!c.contains("RAX"));
    assert!(!c.contains("RCX"));
}

#[test]
fn flags_show_set_and_clear_single_bit_names() {
    let names = vec![
        (0, "CF".to_owned()),
        (6, "ZF".to_owned()),
        (7, "SF".to_owned()),
    ];
    let f = flag_states(&names, 0b0100_0001);
    assert_eq!(
        f,
        vec![
            ("CF".to_owned(), true),
            ("ZF".to_owned(), true),
            ("SF".to_owned(), false)
        ]
    );
}

#[test]
fn a_multi_bit_field_is_not_listed_as_a_flag() {
    // IDA names both bits of IOPL with the same string.
    let names = vec![
        (12, "IOPL".to_owned()),
        (13, "IOPL".to_owned()),
        (14, "NT".to_owned()),
    ];
    let f = flag_states(&names, 0);
    assert_eq!(f, vec![("NT".to_owned(), false)]);
}

#[test]
fn a_dump_row_keeps_unreadable_bytes_and_shows_dots_for_control_bytes() {
    let bytes = vec![Some(b'A'), Some(0), None, Some(b'z')];
    let rows = dump_rows(0x1000, &bytes, 16);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].addr, 0x1000);
    assert_eq!(rows[0].bytes.len(), 4);
    assert_eq!(rows[0].ascii, "A.?z");
}

#[test]
fn a_dump_splits_into_rows_with_a_short_last_row() {
    let bytes: Vec<Option<u8>> = (0..20).map(Some).collect();
    let rows = dump_rows(0x20, &bytes, 16);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].addr, 0x30);
    assert_eq!(rows[1].bytes.len(), 4);
}

#[test]
fn stack_words_read_little_endian_and_flag_unreadable_ones() {
    let mut bytes: Vec<Option<u8>> = vec![
        Some(1),
        Some(0),
        Some(0),
        Some(0),
        Some(0),
        Some(0),
        Some(0),
        Some(0),
    ];
    bytes.extend([
        Some(0xAA),
        None,
        Some(0),
        Some(0),
        Some(0),
        Some(0),
        Some(0),
        Some(0),
    ]);
    bytes.extend([Some(1), Some(2)]); // a truncated tail
    let w = stack_words(0x7000, &bytes, 8);
    assert_eq!(w.len(), 3);
    assert_eq!((w[0].addr, w[0].value), (0x7000, Some(1)));
    assert_eq!((w[1].addr, w[1].value), (0x7008, None));
    assert_eq!((w[2].addr, w[2].value), (0x7010, None));
}

#[test]
fn four_byte_words_for_a_32_bit_target() {
    let bytes: Vec<Option<u8>> = vec![Some(0x78), Some(0x56), Some(0x34), Some(0x12)];
    let w = stack_words(0x100, &bytes, 4);
    assert_eq!(w[0].value, Some(0x1234_5678));
}

#[test]
fn the_pointer_line_is_the_one_with_the_closest_address_at_or_before_it() {
    let lines = vec![vec![], vec![0x10, 0x14], vec![0x20], vec![0x30, 0x2c]];
    assert_eq!(ip_line(&lines, 0x20), Some(2));
    // Between statements: the instruction belongs to the statement before.
    assert_eq!(ip_line(&lines, 0x24), Some(2));
    assert_eq!(ip_line(&lines, 0x2e), Some(3));
}

#[test]
fn no_pointer_line_before_the_first_address_or_without_addresses() {
    let lines = vec![vec![], vec![0x10]];
    assert_eq!(ip_line(&lines, 0x0f), None);
    assert_eq!(ip_line(&[vec![], vec![]], 0x10), None);
}

#[test]
fn two_lines_sharing_the_best_address_pick_the_earlier() {
    let lines = vec![vec![0x10], vec![0x10]];
    assert_eq!(ip_line(&lines, 0x18), Some(0));
}

#[test]
fn a_line_breakpoint_goes_to_the_lowest_address_on_the_line() {
    assert_eq!(line_bpt_ea(&[0x30, 0x2c, 0x34]), Some(0x2c));
    assert_eq!(line_bpt_ea(&[]), None);
}
