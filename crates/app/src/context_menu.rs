//! H1: the entries of the right-click menus (also opened by the Menu key and
//! Ctrl+Alt+M at the caret). They are catalog commands, so labels and shortcut
//! hints come from the live keymap exactly as in the menu bar, plus a few
//! entries that act on the row that was clicked (`Target::Run`).
//!
//! A view builds its entries here and hands them to the workspace through
//! `SessionEvent::ContextMenu`; the menu bar draws them with the same
//! dropdown code, clamped to the window.
use crate::actions;
use crate::commands;
use crate::menubar::{Entry, checked, command_entry, run_entry};
use crate::operands::Ops;
use crate::session::{ContextMenuRequest, Session, SessionEvent};
use gpui::{Action, App, Entity, Pixels, Point, SharedString};
use ida_proto::Ea;

macro_rules! c {
    ($a:ident, $cx:expr) => {
        command_entry(<actions::$a as Action>::name_for_type(), None, true, $cx)
    };
    ($a:ident, $label:expr, $cx:expr) => {
        command_entry(
            <actions::$a as Action>::name_for_type(),
            Some($label),
            true,
            $cx,
        )
    };
}

/// A submenu entry.
pub fn sub(label: &str, entries: Vec<Entry>) -> Entry {
    Entry::Sub {
        label: label.to_owned().into(),
        enabled: true,
        entries,
    }
}

/// Opens a context menu for `session`'s window at `pos`. `keyboard` highlights
/// the first entry (the Menu key); a right click leaves nothing highlighted.
pub fn show(
    session: &Entity<Session>,
    pos: Point<Pixels>,
    title: impl Into<SharedString>,
    entries: Vec<Entry>,
    keyboard: bool,
    cx: &mut App,
) {
    let req = ContextMenuRequest {
        pos,
        title: title.into(),
        entries,
        keyboard,
    };
    session.update(cx, |_, cx| cx.emit(SessionEvent::ContextMenu(req)));
}

/// Edit > Operand types: the representations of the operand under the caret,
/// the current one checked.
pub fn operand_entries(ops: &Ops, cx: &App) -> Vec<Entry> {
    use ida_proto::OpReprKind as K;
    let kind = ops.kind();
    vec![
        c!(OpHexDec, "Number (hex / decimal)", cx),
        checked(c!(OpHex, "Hexadecimal", cx), kind == K::Hex),
        checked(c!(OpDecimal, "Decimal", cx), kind == K::Decimal),
        checked(c!(OpOctal, "Octal", cx), kind == K::Octal),
        checked(c!(OpBinary, "Binary", cx), kind == K::Binary),
        checked(c!(OpChar, "Character", cx), kind == K::Char),
        Entry::Sep,
        checked(c!(OpOffset, "Offset", cx), kind == K::Offset),
        c!(OpOffsetBase, "Offset with base…", cx),
        checked(
            c!(OpStructOffset, "Structure offset…", cx),
            kind == K::StructOffset,
        ),
        checked(c!(OpEnum, "Enum member…", cx), kind == K::Enum),
        checked(c!(OpStackVar, "Stack variable", cx), kind == K::StackVar),
        Entry::Sep,
        checked(c!(OpNegate, "Negate", cx), ops.negated()),
        checked(c!(OpInvert, "Bitwise not", cx), ops.inverted()),
        Entry::Sep,
        c!(OpClear, "Default representation", cx),
    ]
}

/// Edit > Patch program.
pub fn patch_entries(cx: &App) -> Vec<Entry> {
    vec![
        c!(PatchChangeBytes, "Change bytes…", cx),
        c!(PatchAssemble, "Assemble…", cx),
        Entry::Sep,
        c!(PatchRevert, "Revert patches", cx),
        c!(ShowPatches, "Patched bytes…", cx),
        Entry::Sep,
        c!(PatchApply, "Apply patches to input file…", cx),
    ]
}

/// The disassembly and graph menu: annotate, cross references, define, copy.
pub fn listing(ops: &Ops, cx: &App) -> Vec<Entry> {
    vec![
        c!(Rename, cx),
        Entry::Sep,
        c!(XrefsTo, cx),
        c!(XrefsFrom, cx),
        Entry::Sep,
        c!(Comment, cx),
        c!(RepeatableComment, cx),
        c!(SetType, cx),
        Entry::Sep,
        c!(MakeCode, cx),
        c!(MakeData, cx),
        c!(MakeString, cx),
        c!(DefineArray, cx),
        c!(MakeFunction, cx),
        c!(Undefine, cx),
        Entry::Sep,
        sub("Operand types", operand_entries(ops, cx)),
        c!(SetStringType, cx),
        c!(EditFunctionProps, cx),
        Entry::Sep,
        c!(Copy, cx),
        c!(CopyWithAddresses, cx),
        c!(CopyAddress, cx),
    ]
}

/// The pseudocode menu.
pub fn pseudocode(cx: &App) -> Vec<Entry> {
    vec![
        c!(Rename, cx),
        c!(SetType, cx),
        c!(Comment, cx),
        Entry::Sep,
        c!(XrefsTo, cx),
        c!(ShowListing, "Jump to disassembly", cx),
        Entry::Sep,
        c!(Copy, cx),
        c!(CopyWithAddresses, cx),
        c!(CopyAddress, cx),
    ]
}

/// The hex view menu.
pub fn hex(cx: &App) -> Vec<Entry> {
    vec![
        c!(Copy, "Copy as hex", cx),
        c!(CopyAsCArray, "Copy as C array", cx),
        c!(CopyAsPython, "Copy as Python bytes", cx),
        c!(CopyWithAddresses, "Copy as hex dump", cx),
        Entry::Sep,
        c!(GoTo, "Jump to address…", cx),
        c!(CopyAddress, cx),
        Entry::Sep,
        sub("Patch program", patch_entries(cx)),
    ]
}

/// The menu of a row that stands for an address (side lists, inspector and
/// bottom-panel rows): jump, cross references, rename, copy.
pub fn address_row(
    session: &Entity<Session>,
    ea: Ea,
    name: Option<String>,
    cx: &App,
) -> Vec<Entry> {
    let _ = cx;
    let mut out = Vec::new();
    let s = session.clone();
    out.push(run_entry("Jump", "", move |_, cx| {
        s.update(cx, |s, cx| s.navigate(ea, cx))
    }));
    out.push(Entry::Sep);
    let s = session.clone();
    out.push(run_entry("Cross references to", "", move |_, cx| {
        commands::show_xrefs(&s, ea, true, cx)
    }));
    let s = session.clone();
    out.push(run_entry("Rename", "", move |_, cx| {
        commands::rename_ea(&s, ea, cx)
    }));
    out.push(Entry::Sep);
    if let Some(name) = name.filter(|n| !n.is_empty()) {
        out.push(run_entry("Copy name", "", move |_, cx| {
            zui_editor::clipboard::write(name.clone(), cx)
        }));
    }
    out.push(run_entry("Copy address", "", move |_, cx| {
        zui_editor::clipboard::write(format!("{ea:X}"), cx)
    }));
    out
}
