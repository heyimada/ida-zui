// The workspace glue of the editing commands: operand representations
// (Edit > Operand types), arrays, string types, function properties and
// patching. Choosers are pick overlays whose rows run a closure; the dialogs
// live in `edit_dialogs.rs`.
use super::{ModalKind, ModalStyle, Workspace};
use crate::database::Mode;
use crate::edit_dialogs::{
    ApplyPatchesDialog, ArrayDialog, AssembleDialog, BytesDialog, FuncDialog,
};
use crate::jumps::parse_immediate;
use crate::modal::{Choice, ChoiceValue};
use crate::operands::hex_or_decimal;
use gpui::{AppContext, Context, SharedString, Window};
use ida_proto::{
    Ea, MarkDelete, MarkList, MarkSet, OpRepr, PatchList, RevertPatches, SetOpRepr, StringKind,
    TypeList,
};
use std::rc::Rc;

/// The string types of Alt+A, with IDA's names.
const STRING_KINDS: [(StringKind, &str); 12] = [
    (StringKind::C, "C string (8-bit, zero-terminated)"),
    (StringKind::C16, "C string, UTF-16"),
    (StringKind::C32, "C string, UTF-32"),
    (StringKind::Pascal, "Pascal string (1-byte length)"),
    (StringKind::Pascal16, "Pascal string, UTF-16"),
    (StringKind::Pascal32, "Pascal string, UTF-32"),
    (StringKind::Len2, "2-byte length (8-bit)"),
    (StringKind::Len2x16, "2-byte length, UTF-16"),
    (StringKind::Len2x32, "2-byte length, UTF-32"),
    (StringKind::Len4, "4-byte length (8-bit)"),
    (StringKind::Len4x16, "4-byte length, UTF-16"),
    (StringKind::Len4x32, "4-byte length, UTF-32"),
];

impl Workspace {
    /// Alt+M: marks the caret position with a description.
    pub(super) fn mark_position(&mut self, cx: &mut Context<Self>) {
        let (ea, _) = self.edit_target(cx);
        let session = self.session.clone();
        let existing = session
            .read(cx)
            .marks
            .iter()
            .find(|m| m.ea == ea)
            .map(|m| m.desc.clone())
            .unwrap_or_default();
        let s2 = session.clone();
        session.update(cx, |s, cx| {
            s.prompt(
                format!("Mark position {ea:X}: description"),
                existing,
                move |desc, cx| {
                    let fut = s2.read(cx).call(MarkSet { ea, desc });
                    let s3 = s2.clone();
                    cx.spawn(async move |cx| {
                        let result = fut.await;
                        s3.update(cx, |s, cx| match result {
                            Ok(_) => {
                                s.reload_marks(cx);
                                s.info_msg(format!("Marked {ea:X}"), cx);
                            }
                            Err(e) => s.error(e.to_string(), cx),
                        });
                    })
                    .detach();
                },
                cx,
            )
        });
    }

    /// Ctrl+M: a chooser over the marked positions; Enter jumps, Del removes.
    pub(super) fn jump_to_mark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(MarkList {});
        let fut = Box::pin(async move {
            fut.await.map_err(|e| e.to_string()).map(|rows| {
                rows.into_iter()
                    .map(|m| Choice {
                        label: if m.desc.is_empty() {
                            format!("{:X}", m.ea).into()
                        } else {
                            m.desc.into()
                        },
                        detail: format!("{:X}", m.ea).into(),
                        kind: "mark".into(),
                        group: SharedString::default(),
                        keywords: SharedString::default(),
                        value: ChoiceValue::Mark(m.ea, m.slot),
                    })
                    .collect()
            })
        });
        self.pick_ea(
            "Marked positions",
            "No marked positions yet: Alt+M marks the caret",
            false,
            fut,
            window,
            cx,
        );
    }

    /// Del in the marks chooser: removes the mark and shows the rest.
    pub(super) fn delete_mark(&mut self, slot: u32, window: &mut Window, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(MarkDelete { slot });
        let session = self.session.clone();
        let handle = window.window_handle();
        self.close_modal(window, cx);
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            session.update(cx, |s, cx| match &result {
                Ok(()) => s.reload_marks(cx),
                Err(e) => s.error(e.to_string(), cx),
            });
            if result.is_ok() {
                cx.update_window(handle, |_, window, cx| {
                    this.update(cx, |w, cx| w.jump_to_mark(window, cx)).ok();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The item the editing commands act on (its head, as in IDA, even when
    /// the caret came from a jump into the middle of it), and the operand under
    /// the caret.
    fn edit_target(&self, cx: &gpui::App) -> (Ea, u32) {
        let s = self.session.read(cx);
        let head = self
            .focus_ctx
            .read(cx)
            .addr
            .as_ref()
            .filter(|a| a.ea == s.cursor)
            .map_or(s.cursor, |a| a.head);
        (head, s.caret.operand.unwrap_or(0))
    }

    pub(super) fn set_operand(&mut self, repr: OpRepr, cx: &mut Context<Self>) {
        let (ea, op) = self.edit_target(cx);
        self.session.update(cx, |s, cx| {
            s.mutate(
                SetOpRepr {
                    ea,
                    op: Some(op),
                    repr,
                },
                cx,
            )
        });
    }

    /// `H`: hexadecimal, or decimal when it already is hexadecimal.
    pub(super) fn operand_hex_or_decimal(&mut self, cx: &mut Context<Self>) {
        let (_, op) = self.edit_target(cx);
        let repr = hex_or_decimal(&self.session.read(cx).op_repr, op);
        self.set_operand(repr, cx);
    }

    /// Negate and bitwise not are set, not toggled: flip what is shown now.
    pub(super) fn operand_flip(&mut self, negate: bool, cx: &mut Context<Self>) {
        let ops = self.session.read(cx).ops();
        let repr = if negate {
            OpRepr::Negate { on: !ops.negated() }
        } else {
            OpRepr::Invert {
                on: !ops.inverted(),
            }
        };
        self.set_operand(repr, cx);
    }

    /// Offset with a base address typed in.
    pub(super) fn operand_offset_base(&mut self, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        let session = self.session.clone();
        session.update(cx, |s, cx| {
            s.prompt(
                "Offset base address (e.g. 0x140000000)",
                "",
                move |text, cx| {
                    ws.update(cx, |w, cx| match parse_immediate(&text) {
                        Some(base) => w.set_operand(OpRepr::Offset { base: Some(base) }, cx),
                        None => w.session.update(cx, |s, cx| {
                            s.error(format!("'{text}' is not an address"), cx)
                        }),
                    })
                    .ok();
                },
                cx,
            )
        });
    }

    /// `T` / `M`: a chooser over the structures (or enums) of the database.
    pub(super) fn operand_type_chooser(
        &mut self,
        enums: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fut = self.session.read(cx).call(TypeList {});
        let ws = cx.entity().downgrade();
        let fut = Box::pin(async move {
            fut.await.map_err(|e| e.to_string()).map(|list| {
                list.into_iter()
                    .filter(|t| {
                        if enums {
                            t.kind == "enum"
                        } else {
                            matches!(t.kind.as_str(), "struct" | "union")
                        }
                    })
                    .map(|t| {
                        let (ws, name) = (ws.clone(), t.name.clone());
                        Choice {
                            label: t.name.into(),
                            detail: format!("{} members", t.members).into(),
                            kind: t.kind.into(),
                            group: SharedString::default(),
                            keywords: SharedString::default(),
                            value: ChoiceValue::Run(Rc::new(move |cx| {
                                let repr = if enums {
                                    OpRepr::Enum { name: name.clone() }
                                } else {
                                    OpRepr::StructOffset {
                                        path: vec![name.clone()],
                                        delta: 0,
                                    }
                                };
                                ws.update(cx, |w, cx| w.set_operand(repr, cx)).ok();
                            })),
                        }
                    })
                    .collect()
            })
        });
        let (title, empty) = if enums {
            (
                "Enum for the operand",
                "There are no enums in this database",
            )
        } else {
            (
                "Structure for the operand",
                "There are no structures in this database",
            )
        };
        self.pick_ea(title, empty, false, fut, window, cx);
    }

    /// `*`: the array dialog at the caret.
    pub(super) fn define_array(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ea, _) = self.edit_target(cx);
        let session = self.session.clone();
        let dialog = cx.new(|cx| ArrayDialog::new(session, ea, cx));
        self.show_dialog(dialog, window, cx);
    }

    /// Alt+A: a chooser of string types; the string is made at the caret.
    pub(super) fn set_string_type(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ea, _) = self.edit_target(cx);
        let session = self.session.clone();
        let choices: Vec<Choice> = STRING_KINDS
            .iter()
            .map(|(kind, label)| {
                let (session, kind) = (session.clone(), *kind);
                Choice {
                    label: (*label).into(),
                    detail: SharedString::default(),
                    kind: SharedString::default(),
                    group: SharedString::default(),
                    keywords: SharedString::default(),
                    value: ChoiceValue::Run(Rc::new(move |cx| {
                        session.update(cx, |s, cx| {
                            s.mutate(ida_proto::MakeStringTyped { ea, kind, len: 0 }, cx)
                        })
                    })),
                }
            })
            .collect();
        self.open_modal(
            "String type",
            "",
            "Filter",
            Rc::new(choices),
            false,
            ModalKind::Pick,
            ModalStyle::Palette {
                enter: "make string",
                key_hint: None,
            },
            window,
            cx,
        );
    }

    /// Alt+P: the function properties dialog.
    pub(super) fn edit_function(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ea, _) = self.edit_target(cx);
        let session = self.session.clone();
        let dialog = cx.new(|cx| FuncDialog::new(session, ea, cx));
        self.show_dialog(dialog, window, cx);
    }

    /// The bytes the patch commands act on: the hex or listing selection, else
    /// the item at the caret.
    fn patch_span(&self, cx: &gpui::App) -> (Ea, u64) {
        let cursor = self.session.read(cx).cursor;
        if self.doc.is_some() {
            return (cursor, 1);
        }
        match self.mode {
            Mode::Hex => self.hex.read(cx).selection_span(),
            Mode::Linear => self
                .listing
                .read(cx)
                .selection_span()
                .unwrap_or((cursor, 1)),
            _ => (cursor, 1),
        }
    }

    pub(super) fn patch_change_bytes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ea, len) = self.patch_span(cx);
        let session = self.session.clone();
        let len = len.clamp(1, 64) as u32;
        let dialog = cx.new(|cx| BytesDialog::new(session, ea, len, cx));
        self.show_dialog(dialog, window, cx);
    }

    pub(super) fn patch_assemble(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (ea, _) = self.edit_target(cx);
        let session = self.session.clone();
        let dialog = cx.new(|cx| AssembleDialog::new(session, ea, cx));
        self.show_dialog(dialog, window, cx);
    }

    /// Restores the original bytes of the selection (or the item at the caret).
    pub(super) fn patch_revert(&mut self, cx: &mut Context<Self>) {
        let (ea, len) = self.patch_span(cx);
        let fut = self.session.read(cx).call(RevertPatches { ea, len });
        let session = self.session.clone();
        cx.spawn(async move |_, cx| {
            let result = fut.await;
            session.update(cx, |s, cx| match result {
                Ok(0) => s.info_msg("No patched bytes there", cx),
                Ok(n) => {
                    s.changed(cx);
                    s.info_msg(
                        format!("Restored {n} byte{}", if n == 1 { "" } else { "s" }),
                        cx,
                    )
                }
                Err(e) => s.error(e.to_string(), cx),
            });
        })
        .detach();
    }

    /// A chooser of every patched byte; Enter jumps to it.
    pub(super) fn show_patches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(PatchList {});
        let fut = Box::pin(async move {
            fut.await.map_err(|e| e.to_string()).map(|rows| {
                rows.into_iter()
                    .take(5000)
                    .map(|r| Choice {
                        label: format!("{:02X} \u{2192} {:02X}", r.original, r.patched).into(),
                        detail: format!("{:X}", r.ea).into(),
                        kind: "patch".into(),
                        group: SharedString::default(),
                        keywords: SharedString::default(),
                        value: ChoiceValue::Ea(r.ea),
                    })
                    .collect()
            })
        });
        self.pick_ea(
            "Patched bytes",
            "Nothing has been patched",
            false,
            fut,
            window,
            cx,
        );
    }

    /// A patched copy of the input file, never the input itself.
    pub(super) fn patch_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self
            .session
            .read(cx)
            .request
            .as_ref()
            .map(|r| r.input_path());
        let Some(input) = input else {
            self.session.update(cx, |s, cx| {
                s.info_msg("There is no input file to patch", cx)
            });
            return;
        };
        let session = self.session.clone();
        let dialog = cx.new(|cx| ApplyPatchesDialog::new(session, input, cx));
        self.show_dialog(dialog, window, cx);
    }
}
