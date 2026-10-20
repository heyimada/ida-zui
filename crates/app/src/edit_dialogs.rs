// The dialogs behind Edit > Array, Edit function, Patch program (change bytes,
// assemble, apply patches). Each asks the host itself and closes on success;
// an error from the host stays in the dialog so nothing is lost.
use crate::actions::{Cancel, Confirm};
use crate::dialog::{
    ButtonKind, DialogEvent, body, button, caption, checkbox, footer, frame, header,
};
use crate::input::{InputEvent, TextInput};
use crate::operands::{
    array_preview, elem_size, normalize_hex, output_problem, overwrite_note, parse_address,
    parse_count, patched_path,
};
use crate::session::Session;
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use ida_proto::{
    ApplyPatches, ArrayDefaults, ArrayElem, Assemble, Bytes, Ea, EditFunc, FuncEdit, FuncEditInfo,
    FuncEditInfoReply, MakeArray, PatchBytes, Rename,
};
use std::path::PathBuf;

/// One labelled row of a form: the label in a 120 px column, the field beside it.
fn row(theme: &Theme, label: &'static str, field: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(120.))
                .flex_none()
                .text_color(hsla(theme.muted))
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(field))
}

fn error_line(theme: &Theme, error: &Option<SharedString>) -> Option<gpui::Div> {
    error
        .clone()
        .map(|e| div().text_color(hsla(theme.danger)).child(e))
}

// ---- Array ------------------------------------------------------------------

const ARRAY_KINDS: [(&str, fn() -> ArrayElem); 7] = [
    ("Byte", || ArrayElem::Byte),
    ("Word", || ArrayElem::Word),
    ("Dword", || ArrayElem::Dword),
    ("Qword", || ArrayElem::Qword),
    ("Oword", || ArrayElem::Oword),
    ("Float", || ArrayElem::Float),
    ("Double", || ArrayElem::Double),
];

/// `*`: element type and count of an array at the caret, with a preview.
pub struct ArrayDialog {
    session: Entity<Session>,
    ea: Ea,
    elem: ArrayElem,
    /// Size of one element of the structure the host proposed.
    struct_size: Option<u32>,
    max_count: u32,
    count: Entity<TextInput>,
    struct_name: Entity<TextInput>,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for ArrayDialog {}

impl ArrayDialog {
    pub fn new(session: Entity<Session>, ea: Ea, cx: &mut Context<Self>) -> Self {
        let count = cx.new(|cx| TextInput::new("1", "Number of elements", cx).mono());
        let struct_name = cx.new(|cx| TextInput::new("", "Structure name", cx).mono());
        let subs = vec![
            cx.subscribe(&count, |d, _, _: &InputEvent, cx| {
                d.error = None;
                cx.notify();
            }),
            cx.subscribe(&struct_name, |d, _, _: &InputEvent, cx| {
                d.error = None;
                cx.notify();
            }),
        ];
        let fut = session.read(cx).call(ArrayDefaults { ea });
        cx.spawn(async move |this, cx| {
            let reply = fut.await;
            this.update(cx, |d, cx| match reply {
                Ok(r) => {
                    d.max_count = r.max_count;
                    d.struct_size =
                        matches!(r.elem, ArrayElem::Struct { .. }).then_some(r.elem_size);
                    if let ArrayElem::Struct { name } = &r.elem {
                        d.struct_name.update(cx, |i, cx| i.set_value(name, cx));
                    }
                    d.elem = r.elem;
                    d.count
                        .update(cx, |i, cx| i.set_value(&r.count.max(1).to_string(), cx));
                    cx.notify();
                }
                Err(e) => {
                    d.error = Some(e.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        Self {
            session,
            ea,
            elem: ArrayElem::Byte,
            struct_size: None,
            max_count: 0,
            count,
            struct_name,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn element(&self, cx: &App) -> ArrayElem {
        match &self.elem {
            ArrayElem::Struct { .. } => ArrayElem::Struct {
                name: self.struct_name.read(cx).value().trim().to_owned(),
            },
            other => other.clone(),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let count = match parse_count(self.count.read(cx).value()) {
            Ok(n) => n,
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        let elem = self.element(cx);
        if matches!(&elem, ArrayElem::Struct { name } if name.is_empty()) {
            self.error = Some("Enter the name of the structure".into());
            cx.notify();
            return;
        }
        let fut = self.session.read(cx).call(MakeArray {
            ea: self.ea,
            elem,
            count,
        });
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |d, cx| match result {
                Ok(()) => {
                    session.update(cx, |s, cx| s.changed(cx));
                    cx.emit(DialogEvent::Close);
                }
                Err(e) => {
                    d.error = Some(e.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for ArrayDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.count.read(cx).focus_handle(cx)
    }
}

impl Render for ArrayDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let elem = self.element(cx);
        let count = parse_count(self.count.read(cx).value()).unwrap_or(1);
        let preview = array_preview(&elem, self.struct_size, count);
        let note = elem_size(&elem)
            .or(self.struct_size)
            .and_then(|size| overwrite_note(size, count, self.max_count));
        // Two tracks, so the eight choices fit the dialog.
        let mut kinds: (Vec<_>, Vec<_>) = (Vec::new(), Vec::new());
        for (i, (label, make)) in ARRAY_KINDS.iter().enumerate() {
            let on = std::mem::discriminant(&make()) == std::mem::discriminant(&self.elem);
            let seg = ui::segment(("array-kind", i), *label, on, &t).on_click(cx.listener(
                move |d, _, _, cx| {
                    d.elem = make();
                    d.error = None;
                    cx.notify();
                },
            ));
            if i < 4 {
                kinds.0.push(seg);
            } else {
                kinds.1.push(seg);
            }
        }
        let is_struct = matches!(self.elem, ArrayElem::Struct { .. });

        frame(&t, 560.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &t,
                format!("Array at {:X}", self.ea),
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(row(
                        &t,
                        "Element type",
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(ui::segmented_track(&t).children(kinds.0))
                            .child(ui::segmented_track(&t).children(kinds.1).child(
                                ui::segment("array-struct", "Structure", is_struct, &t).on_click(
                                    cx.listener(|d, _, _, cx| {
                                        d.elem = ArrayElem::Struct {
                                            name: String::new(),
                                        };
                                        cx.notify();
                                    }),
                                ),
                            )),
                    ))
                    .when(is_struct, |d| {
                        d.child(row(&t, "Structure", self.struct_name.clone()))
                    })
                    .child(row(&t, "Elements", self.count.clone()))
                    .child(
                        div()
                            .font_family(code_font_name())
                            .text_color(hsla(t.text))
                            .child(preview),
                    )
                    .when_some(note, |d, n| {
                        d.child(div().text_color(hsla(t.warning)).child(n))
                    })
                    .children(error_line(&t, &self.error)),
            )
            .child(
                footer(&t)
                    .child(
                        button("array-cancel", "Cancel", ButtonKind::Secondary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(
                        button("array-ok", "Make array", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|d, _, _, cx| d.submit(cx))),
                    ),
            )
    }
}

// ---- Edit function --------------------------------------------------------------

/// Alt+P: bounds, name, flags and frame sizes of the function at the caret.
pub struct FuncDialog {
    session: Entity<Session>,
    info: Option<FuncEditInfoReply>,
    end: Entity<TextInput>,
    name: Entity<TextInput>,
    locals: Entity<TextInput>,
    saved: Entity<TextInput>,
    purged: Entity<TextInput>,
    flags: Flags,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

#[derive(Clone, Copy, Default)]
struct Flags {
    noret: bool,
    far: bool,
    library: bool,
    static_def: bool,
    thunk: bool,
    bp_frame: bool,
    bp_bottom: bool,
}

impl EventEmitter<DialogEvent> for FuncDialog {}

impl FuncDialog {
    pub fn new(session: Entity<Session>, ea: Ea, cx: &mut Context<Self>) -> Self {
        let field = |ph: &'static str, cx: &mut Context<Self>| {
            cx.new(|cx| TextInput::new("", ph, cx).mono())
        };
        let end = field("End address", cx);
        let name = field("Name", cx);
        let locals = field("Bytes", cx);
        let saved = field("Bytes", cx);
        let purged = field("Bytes", cx);
        let subs = [&end, &name, &locals, &saved, &purged]
            .into_iter()
            .map(|i| {
                cx.subscribe(i, |d, _, _: &InputEvent, cx| {
                    d.error = None;
                    cx.notify();
                })
            })
            .collect();
        let fut = session.read(cx).call(FuncEditInfo { ea });
        cx.spawn(async move |this, cx| {
            let reply = fut.await;
            this.update(cx, |d, cx| {
                match reply {
                    Ok(i) => {
                        d.end
                            .update(cx, |x, cx| x.set_value(&format!("{:X}", i.end), cx));
                        d.name.update(cx, |x, cx| x.set_value(&i.name, cx));
                        d.locals
                            .update(cx, |x, cx| x.set_value(&i.locals.to_string(), cx));
                        d.saved
                            .update(cx, |x, cx| x.set_value(&i.saved_regs.to_string(), cx));
                        d.purged
                            .update(cx, |x, cx| x.set_value(&i.purged.to_string(), cx));
                        d.flags = Flags {
                            noret: i.noret,
                            far: i.far,
                            library: i.library,
                            static_def: i.static_def,
                            thunk: i.thunk,
                            bp_frame: i.bp_frame,
                            bp_bottom: i.bp_bottom,
                        };
                        d.info = Some(i);
                    }
                    Err(e) => d.error = Some(e.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            session,
            info: None,
            end,
            name,
            locals,
            saved,
            purged,
            flags: Flags::default(),
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn number(&self, input: &Entity<TextInput>, what: &str, cx: &App) -> Result<u64, String> {
        let text = input.read(cx).value().trim().to_owned();
        text.parse::<u64>()
            .map_err(|_| format!("{what}: '{text}' is not a number"))
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(info) = self.info.clone() else {
            return;
        };
        let end_text = self.end.read(cx).value().trim().to_owned();
        let end = match parse_address(&end_text) {
            Some(e) => e,
            None => {
                self.error = Some(format!("End: '{end_text}' is not an address").into());
                cx.notify();
                return;
            }
        };
        let numbers = (
            self.number(&self.locals, "Locals", cx),
            self.number(&self.saved, "Saved registers", cx),
            self.number(&self.purged, "Purged bytes", cx),
        );
        let (locals, saved, purged) = match numbers {
            (Ok(a), Ok(b), Ok(c)) => (a, b, c),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        let f = self.flags;
        let edit = FuncEdit {
            end: (end != info.end).then_some(end),
            noret: Some(f.noret),
            far: Some(f.far),
            library: Some(f.library),
            static_def: Some(f.static_def),
            thunk: Some(f.thunk),
            bp_frame: Some(f.bp_frame),
            bp_bottom: Some(f.bp_bottom),
            locals: Some(locals),
            saved_regs: Some(saved as u32),
            purged: Some(purged),
        };
        let name = self.name.read(cx).value().trim().to_owned();
        let s = self.session.read(cx);
        let edit_fut = s.call(EditFunc {
            ea: info.start,
            edit,
        });
        let rename = (name != info.name && !name.is_empty()).then(|| {
            s.call(Rename {
                ea: info.start,
                name,
            })
        });
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let mut result = edit_fut.await.map_err(|e| e.to_string());
            if result.is_ok()
                && let Some(r) = rename
            {
                result = r.await.map_err(|e| e.to_string());
            }
            this.update(cx, |d, cx| match result {
                Ok(()) => {
                    session.update(cx, |s, cx| s.changed(cx));
                    cx.emit(DialogEvent::Close);
                }
                Err(e) => {
                    d.error = Some(e.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for FuncDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.read(cx).focus_handle(cx)
    }
}

impl Render for FuncDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let f = self.flags;
        let flag = |id: &'static str,
                    label: &'static str,
                    on: bool,
                    set: fn(&mut Flags, bool),
                    cx: &mut Context<Self>| {
            crate::dialog::checkbox(id, label, on, &t).on_click(cx.listener(move |d, _, _, cx| {
                set(&mut d.flags, !on);
                cx.notify();
            }))
        };
        let start = self
            .info
            .as_ref()
            .map(|i| format!("{:X}", i.start))
            .unwrap_or_default();
        frame(&t, 600.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &t,
                "Edit function",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(row(
                        &t,
                        "Start",
                        div().font_family(code_font_name()).child(start),
                    ))
                    .child(row(&t, "End", self.end.clone()))
                    .child(row(&t, "Name", self.name.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(16.))
                            .child(flag("f-noret", "Does not return", f.noret, |f, v| f.noret = v, cx))
                            .child(flag("f-far", "Far", f.far, |f, v| f.far = v, cx))
                            .child(flag("f-lib", "Library function", f.library, |f, v| f.library = v, cx))
                            .child(flag("f-static", "Static", f.static_def, |f, v| f.static_def = v, cx))
                            .child(flag("f-thunk", "Thunk", f.thunk, |f, v| f.thunk = v, cx))
                            .child(flag("f-bp", "BP-based frame", f.bp_frame, |f, v| f.bp_frame = v, cx))
                            .child(flag("f-bpb", "BP at the frame bottom", f.bp_bottom, |f, v| f.bp_bottom = v, cx)),
                    )
                    .child(row(&t, "Local variables", self.locals.clone()))
                    .child(row(&t, "Saved registers", self.saved.clone()))
                    .child(row(&t, "Purged bytes", self.purged.clone()))
                    .child(caption(
                        &t,
                        "Sizes are in bytes. The start cannot change here: create a function at the new address instead.",
                    ))
                    .children(error_line(&t, &self.error)),
            )
            .child(
                footer(&t)
                    .child(
                        button("func-cancel", "Cancel", ButtonKind::Secondary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(
                        button("func-ok", "Apply", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|d, _, _, cx| d.submit(cx))),
                    ),
            )
    }
}

// ---- Patch: change bytes ----------------------------------------------------------

/// Edit > Patch program > Change bytes: hex pairs written at the caret.
pub struct BytesDialog {
    session: Entity<Session>,
    ea: Ea,
    hex: Entity<TextInput>,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for BytesDialog {}

impl BytesDialog {
    /// Prefilled with the `len` bytes now at `ea`.
    pub fn new(session: Entity<Session>, ea: Ea, len: u32, cx: &mut Context<Self>) -> Self {
        let hex = cx.new(|cx| {
            TextInput::new("", "Hex pairs, e.g. 90 90 CC", cx)
                .tall()
                .mono()
        });
        let subs = vec![cx.subscribe(&hex, |d, _, _: &InputEvent, cx| {
            d.error = None;
            cx.notify();
        })];
        let fut = session.read(cx).call(Bytes { ea, len });
        cx.spawn(async move |this, cx| {
            if let Ok(chunk) = fut.await {
                this.update(cx, |d, cx| {
                    let hex: Vec<String> = (0..chunk.len())
                        .filter_map(|i| chunk.get(i))
                        .map(|b| format!("{b:02X}"))
                        .collect();
                    d.hex.update(cx, |i, cx| i.set_value(&hex.join(" "), cx));
                })
                .ok();
            }
        })
        .detach();
        Self {
            session,
            ea,
            hex,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let hex = match normalize_hex(self.hex.read(cx).value()) {
            Ok(h) => h,
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
                return;
            }
        };
        let fut = self.session.read(cx).call(PatchBytes { ea: self.ea, hex });
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |d, cx| match result {
                Ok(n) => {
                    session.update(cx, |s, cx| {
                        s.changed(cx);
                        s.info_msg(
                            format!("Patched {n} byte{}", if n == 1 { "" } else { "s" }),
                            cx,
                        )
                    });
                    cx.emit(DialogEvent::Close);
                }
                Err(e) => {
                    d.error = Some(e.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for BytesDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.hex.read(cx).focus_handle(cx)
    }
}

impl Render for BytesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        frame(&t, 600.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &t,
                format!("Change bytes at {:X}", self.ea),
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(self.hex.clone())
                    .child(caption(
                        &t,
                        "The original bytes are kept: Revert patches restores them, and Apply patches writes a copy of the input file.",
                    ))
                    .children(error_line(&t, &self.error)),
            )
            .child(
                footer(&t)
                    .child(
                        button("bytes-cancel", "Cancel", ButtonKind::Secondary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(
                        button("bytes-ok", "Patch", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|d, _, _, cx| d.submit(cx))),
                    ),
            )
    }
}

// ---- Patch: assemble ----------------------------------------------------------------

/// Edit > Patch program > Assemble: one instruction at the caret.
pub struct AssembleDialog {
    session: Entity<Session>,
    ea: Ea,
    text: Entity<TextInput>,
    pad: bool,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for AssembleDialog {}

impl AssembleDialog {
    pub fn new(session: Entity<Session>, ea: Ea, cx: &mut Context<Self>) -> Self {
        let text = cx.new(|cx| {
            TextInput::new("", "Instruction, e.g. mov eax, 1", cx)
                .tall()
                .mono()
        });
        let subs = vec![cx.subscribe(&text, |d, _, _: &InputEvent, cx| {
            d.error = None;
            cx.notify();
        })];
        Self {
            session,
            ea,
            text,
            pad: true,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.text.read(cx).value().trim().to_owned();
        if text.is_empty() {
            self.error = Some("Enter an instruction".into());
            cx.notify();
            return;
        }
        let fut = self.session.read(cx).call(Assemble {
            ea: self.ea,
            text,
            pad: self.pad,
        });
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |d, cx| match result {
                Ok(r) => {
                    session.update(cx, |s, cx| {
                        s.changed(cx);
                        s.info_msg(
                            format!(
                                "Assembled {} byte{}: {}",
                                r.len,
                                if r.len == 1 { "" } else { "s" },
                                r.hex
                            ),
                            cx,
                        )
                    });
                    cx.emit(DialogEvent::Close);
                }
                Err(e) => {
                    d.error = Some(e.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for AssembleDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.text.read(cx).focus_handle(cx)
    }
}

impl Render for AssembleDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        let pad = self.pad;
        frame(&t, 600.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &t,
                format!("Assemble at {:X}", self.ea),
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(self.text.clone())
                    .child(
                        checkbox("asm-pad", "Fill the rest of the old instruction with nop", pad, &t)
                            .on_click(cx.listener(move |d, _, _, cx| {
                                d.pad = !pad;
                                cx.notify();
                            })),
                    )
                    .child(caption(
                        &t,
                        "Uses the processor module's assembler; some processors have none. The bytes written are shown when it succeeds.",
                    ))
                    .children(error_line(&t, &self.error)),
            )
            .child(
                footer(&t)
                    .child(
                        button("asm-cancel", "Cancel", ButtonKind::Secondary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(
                        button("asm-ok", "Assemble", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|d, _, _, cx| d.submit(cx))),
                    ),
            )
    }
}

// ---- Patch: apply to the input file -------------------------------------------------

/// Edit > Patch program > Apply patches to input file: a patched copy, never
/// the input itself. The path is typed or chosen with Browse (the native
/// dialog cannot be automated, so it only sits behind that button).
pub struct ApplyPatchesDialog {
    session: Entity<Session>,
    input: PathBuf,
    path: Entity<TextInput>,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for ApplyPatchesDialog {}

impl ApplyPatchesDialog {
    pub fn new(session: Entity<Session>, input: PathBuf, cx: &mut Context<Self>) -> Self {
        let default = patched_path(&input).display().to_string();
        let path = cx.new(|cx| {
            TextInput::new(&default, "Path of the patched copy", cx)
                .tall()
                .mono()
        });
        let subs = vec![cx.subscribe(&path, |d, _, _: &InputEvent, cx| {
            d.error = None;
            cx.notify();
        })];
        Self {
            session,
            input,
            path,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self
            .input
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let picked = cx.prompt_for_new_path(
            &dir,
            patched_path(&self.input)
                .file_name()
                .and_then(|n| n.to_str()),
        );
        let input = self.path.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(p))) = picked.await {
                input.update(cx, |i, cx| i.set_value(&p.display().to_string(), cx));
            }
        })
        .detach();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let output = PathBuf::from(self.path.read(cx).value().trim().trim_matches('"'));
        if let Some(problem) = output_problem(&self.input, &output) {
            self.error = Some(problem.into());
            cx.notify();
            return;
        }
        let fut = self.session.read(cx).call(ApplyPatches {
            output: output.display().to_string(),
        });
        let session = self.session.clone();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |d, cx| match result {
                Ok(r) => {
                    session.update(cx, |s, cx| {
                        s.info_msg(
                            format!(
                                "Wrote {} patched byte{} to {}{}",
                                r.written,
                                if r.written == 1 { "" } else { "s" },
                                r.output,
                                if r.skipped > 0 {
                                    format!(" ({} not in the file)", r.skipped)
                                } else {
                                    String::new()
                                }
                            ),
                            cx,
                        )
                    });
                    cx.emit(DialogEvent::Close);
                }
                Err(e) => {
                    d.error = Some(e.to_string().into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl Focusable for ApplyPatchesDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.path.read(cx).focus_handle(cx)
    }
}

impl Render for ApplyPatchesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::get(cx);
        frame(&t, 640.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &t,
                "Apply patches to input file",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().child(self.path.clone()))
                            .child(
                                button("patched-browse", "Browse…", ButtonKind::Secondary, &t)
                                    .on_click(cx.listener(|d, _, window, cx| d.browse(window, cx))),
                            ),
                    )
                    .child(caption(
                        &t,
                        format!(
                            "A copy of {} with the patched bytes is written here; the input file itself is never changed.",
                            self.input.display()
                        ),
                    ))
                    .children(error_line(&t, &self.error)),
            )
            .child(
                footer(&t)
                    .child(
                        button("patched-cancel", "Cancel", ButtonKind::Secondary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    )
                    .child(
                        button("patched-ok", "Write copy", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|d, _, _, cx| d.submit(cx))),
                    ),
            )
    }
}
