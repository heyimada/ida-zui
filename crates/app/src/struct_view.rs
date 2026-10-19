// The structure / union editor: a table of members with inline editing, and
// a second mode that shows the same type as C text in the code editor.
use crate::actions::*;
use crate::docs::DocEvent;
use crate::fields::split_field;
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::table::{self, Col, InlineEdit};
use crate::theme::{Theme, code_font_name, code_font_size, code_line_height, hsla};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{
    TypeApply, TypeDecl, UdtAdd, UdtComment, UdtDelete, UdtInfo, UdtInfoReply, UdtMember,
    UdtRename, UdtRetype, UdtXrefs,
};
use std::rc::Rc;
use zui_editor::{CHighlighter, Editor, EditorEvent, ErrorMarker};

const COLS: [Col; 5] = [
    Col::fixed("Offset", 90.),
    Col::flex("Name"),
    Col::flex("Type"),
    Col::fixed("Size", 70.),
    Col::flex("Comment").text(),
];
const NAME: usize = 1;
const TYPE: usize = 2;
const COMMENT: usize = 4;

/// What `D` cycles a field through, like IDA's byte / word / dword / qword.
const SIZES: [(u64, &str); 4] = [(1, "_BYTE"), (2, "_WORD"), (4, "_DWORD"), (8, "_QWORD")];

pub struct StructView {
    session: Entity<Session>,
    name: String,
    info: Option<Rc<UdtInfoReply>>,
    row: usize,
    scroll: UniformListScrollHandle,
    edit: Option<InlineEdit>,
    decl_mode: bool,
    editor: Entity<Editor>,
    loaded: String,
    status: Option<(SharedString, bool)>,
    style: (&'static str, SharedString, u32),
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DocEvent> for StructView {}

impl StructView {
    pub fn new(session: Entity<Session>, name: String, cx: &mut Context<Self>) -> Self {
        let theme = Theme::get(cx);
        let editor = cx.new(|cx| {
            let mut e = Editor::new(cx)
                .with_highlighter(CHighlighter)
                .with_theme(theme.editor())
                .with_font(
                    code_font_name(),
                    px(code_font_size()),
                    px(code_line_height()),
                );
            e.set_own_scrollbars(false);
            e
        });
        let subs = vec![
            cx.subscribe(&editor, |this, _, ev: &EditorEvent, cx| match ev {
                EditorEvent::Submit => this.apply_decl(cx),
                _ => cx.notify(),
            }),
            cx.subscribe(&session, |this, _, ev, cx| {
                if matches!(ev, SessionEvent::Changed) {
                    this.reload(cx);
                }
            }),
        ];
        let mut me = Self {
            session,
            name,
            info: None,
            row: 0,
            scroll: UniformListScrollHandle::new(),
            edit: None,
            decl_mode: false,
            editor,
            loaded: String::new(),
            status: None,
            style: (theme.name, code_font_name(), code_font_size().to_bits()),
            focus: cx.focus_handle(),
            _subs: subs,
        };
        me.reload(cx);
        me
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let name = self.name.clone();
        let info = self.session.read(cx).call(UdtInfo { name: name.clone() });
        let decl = self.session.read(cx).call(TypeDecl { name });
        cx.spawn(async move |this, cx| {
            let info = info.await;
            let decl = decl.await;
            this.update(cx, |v, cx| {
                match info {
                    Ok(i) => {
                        v.row = v.row.min(i.members.len().saturating_sub(1));
                        v.name = i.name.clone();
                        v.info = Some(Rc::new(i));
                        cx.emit(DocEvent::Title(v.name.clone().into()));
                    }
                    Err(_) => {
                        // Deleted or undone away.
                        v.info = None;
                        v.status = Some((format!("{} no longer exists", v.name).into(), true));
                    }
                }
                if let Ok(d) = decl {
                    let dirty = v.editor.read(cx).text() != v.loaded;
                    if !dirty {
                        v.loaded = d.decl.clone();
                        v.editor.update(cx, |e, cx| e.set_text(&d.decl, cx));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn members(&self) -> &[UdtMember] {
        self.info.as_ref().map_or(&[], |i| &i.members)
    }

    fn current(&self) -> Option<&UdtMember> {
        self.members().get(self.row)
    }

    fn mv(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.members().len();
        if n == 0 {
            return;
        }
        self.row = (self.row as isize + delta).clamp(0, n as isize - 1) as usize;
        self.scroll
            .scroll_to_item(self.row, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        self.edit = None;
        self.row = row;
        cx.notify();
    }

    fn cell_text(m: &UdtMember, col: usize) -> String {
        match col {
            NAME => m.name.clone(),
            TYPE => m.ty.clone(),
            COMMENT => m.cmt.clone(),
            _ => String::new(),
        }
    }

    fn begin_edit(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.members().get(row).cloned() else {
            return;
        };
        self.row = row;
        if m.gap {
            // Undefined bytes: defining a field is the only edit.
            self.add_field(Some(m.offset), cx);
            return;
        }
        let text = Self::cell_text(&m, col);
        self.edit = Some(InlineEdit::start(
            row,
            col,
            &text,
            |v: &mut Self, _, cx| v.cancel_edit(cx),
            window,
            cx,
        ));
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        if self.edit.take().is_some() {
            cx.notify();
        }
    }

    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.edit.take() else { return };
        let text = edit.text(cx);
        window.focus(&self.focus, cx);
        let Some(m) = self.members().get(edit.row).cloned() else {
            return;
        };
        let name = self.name.clone();
        let index = m.index;
        if Self::cell_text(&m, edit.col) == text {
            cx.notify();
            return;
        }
        self.session.update(cx, |s, cx| match edit.col {
            NAME => s.mutate(
                UdtRename {
                    name,
                    index,
                    new: text,
                },
                cx,
            ),
            TYPE => s.mutate(
                UdtRetype {
                    name,
                    index,
                    decl: text,
                    force: true,
                },
                cx,
            ),
            _ => s.mutate(UdtComment { name, index, text }, cx),
        });
        cx.notify();
    }

    /// `D`: cycle the size of the field, or define one on undefined bytes.
    fn cycle_size(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.current().cloned() else {
            return;
        };
        let name = self.name.clone();
        let next = if m.gap {
            SIZES[0].1
        } else {
            let at = SIZES.iter().position(|(s, _)| *s == m.size);
            SIZES[at.map_or(0, |i| (i + 1) % SIZES.len())].1
        };
        self.session.update(cx, |s, cx| {
            if m.gap {
                s.mutate(
                    UdtAdd {
                        name,
                        member: format!("field_{:X}", m.offset),
                        decl: next.into(),
                        offset: Some(m.offset),
                    },
                    cx,
                );
            } else {
                s.mutate(
                    UdtRetype {
                        name,
                        index: m.index,
                        decl: next.into(),
                        force: true,
                    },
                    cx,
                );
            }
        });
    }

    fn delete_member(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.current().cloned() else {
            return;
        };
        if m.gap {
            return;
        }
        let name = self.name.clone();
        self.session.update(cx, |s, cx| {
            s.mutate(
                UdtDelete {
                    name,
                    index: m.index,
                },
                cx,
            )
        });
    }

    /// Prompts for `type name` (C syntax) and adds it at `offset` (the end by default).
    fn add_field(&mut self, offset: Option<u64>, cx: &mut Context<Self>) {
        let name = self.name.clone();
        let session = self.session.clone();
        let at = offset.unwrap_or_else(|| self.info.as_ref().map_or(0, |i| i.size));
        let initial = format!("int field_{at:X}");
        self.session.update(cx, |s, cx| {
            s.prompt(
                format!("Add field at +{at:#X}: type and name, e.g. char buf[16]"),
                initial,
                move |text, cx| match split_field(&text) {
                    Some((decl, member)) => session.update(cx, |s, cx| {
                        s.mutate(
                            UdtAdd {
                                name: name.clone(),
                                member,
                                decl,
                                offset: Some(at),
                            },
                            cx,
                        )
                    }),
                    None => session.update(cx, |s, cx| {
                        s.error(format!("'{text}' is not a field declaration"), cx)
                    }),
                },
                cx,
            )
        });
    }

    fn xrefs(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.current().cloned() else {
            return;
        };
        let fut = self.session.read(cx).call(UdtXrefs {
            name: self.name.clone(),
            index: m.index,
        });
        let title = format!("Cross references to {}.{}", self.name, m.name);
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                v.session.update(cx, |s, cx| match r {
                    Ok(rows) => cx.emit(SessionEvent::ShowXrefList { title, rows }),
                    Err(e) => s.error(e.to_string(), cx),
                })
            })
            .ok();
        })
        .detach();
    }

    fn apply_decl(&mut self, cx: &mut Context<Self>) {
        let decl = self.editor.read(cx).text();
        let fut = self.session.read(cx).call(TypeApply { decl: decl.clone() });
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| match r {
                Ok(reply) => {
                    let errors = reply.errors(&decl);
                    if reply.errors > 0 {
                        let markers = errors
                            .iter()
                            .map(|e| ErrorMarker::new(e.line, e.line, e.message.clone()))
                            .collect();
                        v.editor.update(cx, |e, cx| e.set_errors(markers, cx));
                        let first = errors
                            .first()
                            .map(|e| format!("line {}: {}", e.line + 1, e.message))
                            .unwrap_or_default();
                        v.status =
                            Some((format!("{} error(s): {first}", reply.errors).into(), true));
                    } else {
                        v.status = Some(("Applied".into(), false));
                        v.loaded = decl.clone();
                    }
                    if reply.applied || !reply.names.is_empty() {
                        v.session.update(cx, |s, cx| s.changed(cx));
                    }
                    cx.notify();
                }
                Err(e) => v.session.update(cx, |s, cx| s.error(e.to_string(), cx)),
            })
            .ok();
        })
        .detach();
    }

    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let theme = Theme::get(cx);
        let now = (theme.name, code_font_name(), code_font_size().to_bits());
        if now != self.style {
            self.style = now;
            self.editor.update(cx, |e, cx| {
                e.set_theme(theme.editor(), cx);
                e.set_font(
                    code_font_name(),
                    px(code_font_size()),
                    px(code_line_height()),
                    cx,
                );
            });
        }
    }

    fn render_table(
        &self,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let info = self.info.clone();
        let count = info.as_ref().map_or(0, |i| i.members.len());
        let selected = self.row;
        let entity = cx.entity();
        let edit = self.edit.as_ref().map(|e| (e.row, e.col, e.input.clone()));
        let list = uniform_list("struct-rows", count, move |range, _w, _cx| {
            let Some(info) = info.as_ref() else {
                return Vec::new();
            };
            range
                .map(|ix| {
                    let m = &info.members[ix];
                    let entity = entity.clone();
                    let offset = match m.bit_offset {
                        Some(b) => format!("+{:#06X}.{b}", m.offset),
                        None => format!("+{:#06X}", m.offset),
                    };
                    let size = match m.bits {
                        Some(b) => format!("{b} bit"),
                        None => format!("{:#X}", m.size),
                    };
                    let name = if m.gap {
                        format!("({} undefined)", m.size)
                    } else {
                        m.name.clone()
                    };
                    let cells: [(usize, SharedString); 5] = [
                        (0, offset.into()),
                        (NAME, name.into()),
                        (
                            TYPE,
                            if m.gap {
                                SharedString::default()
                            } else {
                                m.ty.clone().into()
                            },
                        ),
                        (3, size.into()),
                        (COMMENT, m.cmt.clone().into()),
                    ];
                    table::row_shell(("srow", ix), ix == selected, m.gap, &theme)
                        .on_click(move |ev, window, cx| {
                            entity.update(cx, |v, cx| {
                                v.select(ix, cx);
                                if ev.click_count() >= 2 {
                                    v.begin_edit(ix, NAME, window, cx);
                                }
                            })
                        })
                        .children(cells.into_iter().map(|(c, text)| {
                            let cell = table::cell(&COLS[c]);
                            match &edit {
                                Some((r, ec, input)) if *r == ix && *ec == c => {
                                    cell.px(px(2.)).child(input.clone())
                                }
                                _ => cell.child(text),
                            }
                        }))
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();
        let bar = scrollbar::scrollbar(
            "table-scroll",
            Source::list(&self.scroll),
            Axes::V,
            &theme,
            window,
            cx,
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(table::header(&COLS, &theme))
            .child(scrollbar::area(list, bar))
    }
}

impl Focusable for StructView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for StructView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_style(cx);
        let theme = Theme::get(cx);
        let title = self
            .info
            .as_ref()
            .map(|i| {
                format!(
                    "{} {}  ·  {:#X} bytes  ·  {} members",
                    if i.is_union { "union" } else { "struct" },
                    i.name,
                    i.size,
                    i.members.iter().filter(|m| !m.gap).count()
                )
            })
            .unwrap_or_else(|| self.name.clone());
        let editing = self.edit.is_some();
        let decl_mode = self.decl_mode;
        let status = self.status.clone();
        let body = if decl_mode {
            let bar = scrollbar::scrollbar(
                "decl-scroll",
                scrollbar::editor_source(&self.editor),
                Axes::BOTH,
                &theme,
                window,
                cx,
            );
            div()
                .size_full()
                .relative()
                .child(self.editor.clone())
                .child(bar)
                .into_any_element()
        } else {
            self.render_table(theme, window, cx).into_any_element()
        };
        div()
            .id("struct-view")
            .track_focus(&self.focus)
            // Single-letter keys are commands, except while a cell or the text editor has the keyboard.
            .when(!editing && !decl_mode, |d| d.key_context("CodeView"))
            .when(editing, |d| d.key_context("CellEdit"))
            .when(decl_mode, |d| d.key_context("TypeEditor"))
            .on_action(cx.listener(|v, _: &ApplyDecl, _, cx| v.apply_decl(cx)))
            .on_action(cx.listener(|v, _: &Confirm, window, cx| v.commit_edit(window, cx)))
            .on_action(cx.listener(|v, _: &Cancel, window, cx| {
                v.cancel_edit(cx);
                window.focus(&v.focus, cx);
            }))
            .on_action(cx.listener(|v, _: &CursorUp, _, cx| v.mv(-1, cx)))
            .on_action(cx.listener(|v, _: &CursorDown, _, cx| v.mv(1, cx)))
            .on_action(cx.listener(|v, _: &PageUp, _, cx| v.mv(-10, cx)))
            .on_action(cx.listener(|v, _: &PageDown, _, cx| v.mv(10, cx)))
            .on_action(cx.listener(|v, _: &Top, _, cx| v.mv(-100000, cx)))
            .on_action(cx.listener(|v, _: &Bottom, _, cx| v.mv(100000, cx)))
            .on_action(cx.listener(|v, _: &Follow, window, cx| {
                let r = v.row;
                v.begin_edit(r, NAME, window, cx)
            }))
            .on_action(cx.listener(|v, _: &Rename, window, cx| {
                let r = v.row;
                v.begin_edit(r, NAME, window, cx)
            }))
            .on_action(cx.listener(|v, _: &SetType, window, cx| {
                let r = v.row;
                v.begin_edit(r, TYPE, window, cx)
            }))
            .on_action(cx.listener(|v, _: &Comment, window, cx| {
                let r = v.row;
                v.begin_edit(r, COMMENT, window, cx)
            }))
            .on_action(cx.listener(|v, _: &MakeData, _, cx| v.cycle_size(cx)))
            .on_action(cx.listener(|v, _: &Undefine, _, cx| v.delete_member(cx)))
            .on_action(cx.listener(|v, _: &DeleteMember, _, cx| v.delete_member(cx)))
            .on_action(cx.listener(|v, _: &AddField, _, cx| v.add_field(None, cx)))
            .on_action(cx.listener(|v, _: &XrefsTo, _, cx| v.xrefs(cx)))
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface))
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(hsla(theme.border))
                    .child(div().font_family(code_font_name()).text_size(px(13.)).child(title))
                    .child(div().flex_1())
                    .when(decl_mode, |d| {
                        d.child(table::primary_button("apply", "Apply  Ctrl+S", &theme).on_click(cx.listener(|v, _, _, cx| v.apply_decl(cx))))
                    })
                    .when(!decl_mode, |d| {
                        d.child(table::button("add", "Add field…  Ins", &theme).on_click(cx.listener(|v, _, _, cx| v.add_field(None, cx))))
                    })
                    .child(
                        table::button("mode", if decl_mode { "Table" } else { "C declaration" }, &theme).on_click(cx.listener(|v, _, window, cx| {
                            v.decl_mode = !v.decl_mode;
                            if v.decl_mode {
                                let f = v.editor.read(cx).focus_handle(cx);
                                window.focus(&f, cx);
                            } else {
                                window.focus(&v.focus, cx);
                            }
                            cx.notify();
                        })),
                    )
                    .child(table::button("types", "Local types", &theme).on_click(cx.listener(|v, _, _, cx| {
                        let name = v.name.clone();
                        cx.emit(DocEvent::Open(crate::docs::DocKey::LocalTypes(Some(name))))
                    }))),
            )
            .child(div().flex_1().min_h_0().child(body))
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .gap(px(14.))
                    .border_t_1()
                    .border_color(hsla(theme.border))
                    .bg(hsla(theme.surface2))
                    .text_size(px(11.5))
                    .text_color(hsla(theme.muted))
                    .child(match status {
                        Some((t, err)) => div().text_color(if err { hsla(theme.danger) } else { hsla(theme.success) }).child(t),
                        None => div().child("N rename   Y retype   ; comment   D cycle size   U / Del delete   X xrefs   Ins add   double-click edits"),
                    }),
            )
    }
}
