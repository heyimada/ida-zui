// The stack frame editor: the function's frame as a table, split into
// locals, saved registers, the return address and arguments, with the
// undefined bytes between variables shown as rows you can define.
use crate::actions::*;
use crate::docs::DocEvent;
use crate::fields::split_field;
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::table::{self, Col, InlineEdit};
use crate::theme::{Theme, code_font_name, hsla};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{
    Ea, FrameComment, FrameDefine, FrameDelete, FrameInfo, FrameInfoReply, FrameMember,
    FrameRename, FrameRetype, FrameXrefs,
};
use std::rc::Rc;

const COLS: [Col; 6] = [
    Col::fixed("SP+", 76.),
    Col::fixed("FP±", 76.),
    Col::flex("Name"),
    Col::flex("Type"),
    Col::fixed("Size", 60.),
    Col::flex("Comment").text(),
];
const NAME: usize = 2;
const TYPE: usize = 3;
const COMMENT: usize = 5;

const SIZES: [(u64, &str); 4] = [(1, "_BYTE"), (2, "_WORD"), (4, "_DWORD"), (8, "_QWORD")];

#[derive(Clone)]
enum Row {
    /// A region heading.
    Sep(&'static str, u64),
    Member(FrameMember),
    /// Undefined bytes at `offset`.
    Gap {
        offset: u64,
        size: u64,
        fp_off: i64,
    },
}

/// The table rows: regions in frame order, members inside them, gaps between.
fn build_rows(f: &FrameInfoReply) -> Vec<Row> {
    let regions = [
        ("Local variables", f.lvars),
        ("Saved registers", f.saved_regs),
        ("Return address", f.return_address),
        ("Arguments", f.args),
    ];
    let fp = |off: u64| off as i64 - f.locals_size as i64 + f.fpd;
    let mut rows = Vec::new();
    for (label, r) in regions {
        if r.end <= r.start {
            continue;
        }
        rows.push(Row::Sep(label, r.end - r.start));
        let mut cur = r.start;
        for m in f
            .members
            .iter()
            .filter(|m| m.offset >= r.start && m.offset < r.end)
        {
            if m.offset > cur {
                rows.push(Row::Gap {
                    offset: cur,
                    size: m.offset - cur,
                    fp_off: fp(cur),
                });
            }
            rows.push(Row::Member(m.clone()));
            cur = cur.max(m.offset + m.size.max(1));
        }
        if cur < r.end {
            rows.push(Row::Gap {
                offset: cur,
                size: r.end - cur,
                fp_off: fp(cur),
            });
        }
    }
    rows
}

fn signed(v: i64) -> String {
    format!("{}{:X}", if v < 0 { '-' } else { '+' }, v.unsigned_abs())
}

pub struct FrameView {
    session: Entity<Session>,
    func: Ea,
    info: Option<Rc<FrameInfoReply>>,
    rows: Rc<Vec<Row>>,
    row: usize,
    scroll: UniformListScrollHandle,
    edit: Option<InlineEdit>,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<DocEvent> for FrameView {}

impl FrameView {
    pub fn new(session: Entity<Session>, func: Ea, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| {
            if matches!(ev, SessionEvent::Changed) {
                this.reload(cx);
            }
        });
        let mut me = Self {
            session,
            func,
            info: None,
            rows: Rc::default(),
            row: 0,
            scroll: UniformListScrollHandle::new(),
            edit: None,
            focus: cx.focus_handle(),
            _sub: sub,
        };
        me.reload(cx);
        me
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(FrameInfo { func: self.func });
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                match r {
                    Ok(i) => {
                        v.rows = Rc::new(build_rows(&i));
                        v.row = v.row.min(v.rows.len().saturating_sub(1));
                        cx.emit(DocEvent::Title(format!("Frame: {}", i.name).into()));
                        v.info = Some(Rc::new(i));
                    }
                    Err(_) => {
                        v.info = None;
                        v.rows = Rc::default();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn mv(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        self.row = (self.row as isize + delta).clamp(0, n as isize - 1) as usize;
        self.scroll
            .scroll_to_item(self.row, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn member_at(&self, row: usize) -> Option<FrameMember> {
        match self.rows.get(row)? {
            Row::Member(m) => Some(m.clone()),
            _ => None,
        }
    }

    fn begin_edit(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.row = row;
        match self.rows.get(row).cloned() {
            Some(Row::Member(m)) if m.kind != "return_address" => {
                let text = match col {
                    NAME => m.name,
                    TYPE => m.ty,
                    _ => m.cmt,
                };
                self.edit = Some(InlineEdit::start(
                    row,
                    col,
                    &text,
                    |v: &mut Self, _, cx| v.cancel_edit(cx),
                    window,
                    cx,
                ));
            }
            Some(Row::Gap { offset, .. }) => self.define(offset, cx),
            _ => {}
        }
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
        let Some(m) = self.member_at(edit.row) else {
            return;
        };
        let func = self.func;
        let offset = m.offset;
        self.session.update(cx, |s, cx| match edit.col {
            NAME => s.mutate(
                FrameRename {
                    func,
                    offset,
                    new: text,
                },
                cx,
            ),
            TYPE => s.mutate(
                FrameRetype {
                    func,
                    offset,
                    decl: text,
                    force: true,
                },
                cx,
            ),
            _ => s.mutate(FrameComment { func, offset, text }, cx),
        });
        cx.notify();
    }

    /// `D`: cycle the size of a variable, or define one on undefined bytes.
    fn cycle_size(&mut self, cx: &mut Context<Self>) {
        let func = self.func;
        match self.rows.get(self.row).cloned() {
            Some(Row::Gap { offset, .. }) => self.session.update(cx, |s, cx| {
                s.mutate(
                    FrameDefine {
                        func,
                        offset,
                        name: String::new(),
                        decl: SIZES[0].1.into(),
                    },
                    cx,
                )
            }),
            Some(Row::Member(m)) if m.kind != "return_address" => {
                let at = SIZES.iter().position(|(s, _)| *s == m.size);
                let decl = SIZES[at.map_or(0, |i| (i + 1) % SIZES.len())].1.to_owned();
                self.session.update(cx, |s, cx| {
                    s.mutate(
                        FrameRetype {
                            func,
                            offset: m.offset,
                            decl,
                            force: true,
                        },
                        cx,
                    )
                });
            }
            _ => {}
        }
    }

    fn delete_member(&mut self, cx: &mut Context<Self>) {
        if let Some(m) = self.member_at(self.row) {
            let func = self.func;
            self.session.update(cx, |s, cx| {
                s.mutate(
                    FrameDelete {
                        func,
                        offset: m.offset,
                    },
                    cx,
                )
            });
        }
    }

    /// Prompts for `type name` and defines it at the selected undefined bytes.
    fn define(&mut self, offset: u64, cx: &mut Context<Self>) {
        let func = self.func;
        let session = self.session.clone();
        self.session.update(cx, |s, cx| {
            s.prompt(
                format!(
                    "Define a variable at frame offset +{offset:#X}: type and name, e.g. int count"
                ),
                format!("int var_{offset:X}"),
                move |text, cx| match split_field(&text) {
                    Some((decl, name)) => session.update(cx, |s, cx| {
                        s.mutate(
                            FrameDefine {
                                func,
                                offset,
                                name,
                                decl,
                            },
                            cx,
                        )
                    }),
                    None => session.update(cx, |s, cx| {
                        s.error(format!("'{text}' is not a variable declaration"), cx)
                    }),
                },
                cx,
            )
        });
    }

    fn add_field(&mut self, cx: &mut Context<Self>) {
        match self.rows.get(self.row) {
            Some(Row::Gap { offset, .. }) => {
                let o = *offset;
                self.define(o, cx)
            }
            _ => self.session.update(cx, |s, cx| {
                s.info_msg(
                    "Select a row of undefined bytes to define a variable there",
                    cx,
                )
            }),
        }
    }

    fn xrefs(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.member_at(self.row) else {
            return;
        };
        let fut = self.session.read(cx).call(FrameXrefs {
            func: self.func,
            offset: m.offset,
        });
        let title = format!("Cross references to {}", m.name);
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

    fn render_table(
        &self,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rows = self.rows.clone();
        let selected = self.row;
        let entity = cx.entity();
        let edit = self.edit.as_ref().map(|e| (e.row, e.col, e.input.clone()));
        let list = uniform_list("frame-rows", rows.len(), move |range, _w, _cx| {
            range
                .map(|ix| match &rows[ix] {
                    Row::Sep(label, size) => div()
                        .id(("frow", ix))
                        .w_full()
                        .h(px(table::ROW_HEIGHT))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(10.))
                        .bg(hsla(theme.surface2))
                        .border_t_1()
                        .border_color(hsla(theme.border))
                        .text_size(px(11.))
                        .text_color(hsla(theme.accent))
                        .child(*label)
                        .child(
                            div()
                                .text_color(hsla(theme.faint))
                                .child(format!("{size} bytes")),
                        )
                        .into_any_element(),
                    row => {
                        let (offset, fp_off, name, ty, size, cmt, muted) = match row {
                            Row::Member(m) => (
                                m.offset,
                                m.fp_off,
                                m.name.clone(),
                                m.ty.clone(),
                                m.size,
                                m.cmt.clone(),
                                false,
                            ),
                            Row::Gap {
                                offset,
                                size,
                                fp_off,
                            } => (
                                *offset,
                                *fp_off,
                                format!("({size} undefined)"),
                                String::new(),
                                *size,
                                String::new(),
                                true,
                            ),
                            Row::Sep(..) => unreachable!(),
                        };
                        let entity = entity.clone();
                        let cells: [(usize, SharedString); 6] = [
                            (0, format!("{offset:04X}").into()),
                            (1, signed(fp_off).into()),
                            (NAME, name.into()),
                            (TYPE, ty.into()),
                            (4, format!("{size:#X}").into()),
                            (COMMENT, cmt.into()),
                        ];
                        table::row_shell(("frow", ix), ix == selected, muted, &theme)
                            .on_click(move |ev, window, cx| {
                                entity.update(cx, |v, cx| {
                                    v.edit = None;
                                    v.row = ix;
                                    if ev.click_count() >= 2 {
                                        v.begin_edit(ix, NAME, window, cx);
                                    }
                                    cx.notify();
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
                            .into_any_element()
                    }
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

impl Focusable for FrameView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FrameView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let title = match &self.info {
            Some(i) => format!("{}  ·  frame {:#X} bytes", i.name, i.size),
            None => "No stack frame".to_owned(),
        };
        let parts = self.info.as_ref().map(|i| {
            format!(
                "locals {:#X}   saved registers {:#X}   return address {:#X}   arguments {:#X}",
                i.locals_size, i.saved_regs_size, i.ret_size, i.args_size
            )
        });
        let editing = self.edit.is_some();
        div()
            .id("frame-view")
            .track_focus(&self.focus)
            .when(!editing, |d| d.key_context("CodeView"))
            .when(editing, |d| d.key_context("CellEdit"))
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
            .on_action(cx.listener(|v, _: &AddField, _, cx| v.add_field(cx)))
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
                    .gap(px(14.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(hsla(theme.border))
                    .child(div().font_family(code_font_name()).text_size(px(13.)).child(title))
                    .when_some(parts, |d, p| d.child(div().text_size(px(11.5)).text_color(hsla(theme.muted)).child(p))),
            )
            .child(div().flex_1().min_h_0().child(self.render_table(theme, window, cx)))
            .child(
                div()
                    .h(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .border_t_1()
                    .border_color(hsla(theme.border))
                    .bg(hsla(theme.surface2))
                    .text_size(px(11.5))
                    .text_color(hsla(theme.muted))
                    .child("N rename   Y retype   ; comment   D define / cycle size   U / Del delete   X xrefs   Ins define at undefined bytes"),
            )
    }
}
