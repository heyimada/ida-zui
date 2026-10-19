// The enumeration editor: members with their values (hex or decimal), inline
// editing, and the bit-flag switch.
use crate::actions::*;
use crate::docs::{DocEvent, DocKey};
use crate::fields::{format_value, parse_value};
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::table::{self, Col, InlineEdit};
use crate::theme::{Theme, code_font_name, hsla};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{
    EnumAdd, EnumComment, EnumDelete, EnumInfo, EnumInfoReply, EnumRename, EnumSetBitfield,
    EnumSetValue,
};
use std::rc::Rc;

const COLS: [Col; 3] = [
    Col::flex("Name"),
    Col::fixed("Value", 200.),
    Col::flex("Comment").text(),
];
const NAME: usize = 0;
const VALUE: usize = 1;
const COMMENT: usize = 2;

pub struct EnumView {
    session: Entity<Session>,
    name: String,
    info: Option<Rc<EnumInfoReply>>,
    row: usize,
    hex: bool,
    scroll: UniformListScrollHandle,
    edit: Option<InlineEdit>,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<DocEvent> for EnumView {}

impl EnumView {
    pub fn new(session: Entity<Session>, name: String, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&session, |this, _, ev, cx| {
            if matches!(ev, SessionEvent::Changed) {
                this.reload(cx);
            }
        });
        let mut me = Self {
            session,
            name,
            info: None,
            row: 0,
            hex: true,
            scroll: UniformListScrollHandle::new(),
            edit: None,
            focus: cx.focus_handle(),
            _sub: sub,
        };
        me.reload(cx);
        me
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(EnumInfo {
            name: self.name.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                match r {
                    Ok(i) => {
                        v.row = v.row.min(i.members.len().saturating_sub(1));
                        v.name = i.name.clone();
                        v.info = Some(Rc::new(i));
                        cx.emit(DocEvent::Title(v.name.clone().into()));
                    }
                    Err(_) => v.info = None,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn count(&self) -> usize {
        self.info.as_ref().map_or(0, |i| i.members.len())
    }

    fn mv(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.count() == 0 {
            return;
        }
        self.row = (self.row as isize + delta).clamp(0, self.count() as isize - 1) as usize;
        self.scroll
            .scroll_to_item(self.row, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn begin_edit(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.info.as_ref().and_then(|i| i.members.get(row)).cloned() else {
            return;
        };
        self.row = row;
        let text = match col {
            NAME => m.name,
            VALUE => format_value(m.value, self.hex),
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
        let Some(m) = self
            .info
            .as_ref()
            .and_then(|i| i.members.get(edit.row))
            .cloned()
        else {
            return;
        };
        let name = self.name.clone();
        let index = m.index;
        self.session.update(cx, |s, cx| match edit.col {
            NAME => s.mutate(
                EnumRename {
                    name,
                    index,
                    new: text,
                },
                cx,
            ),
            VALUE => match parse_value(&text) {
                Some(value) => s.mutate(EnumSetValue { name, index, value }, cx),
                None => s.error(
                    format!("'{text}' is not a number (decimal, 0x hex, 0b binary or 1<<n)"),
                    cx,
                ),
            },
            _ => s.mutate(EnumComment { name, index, text }, cx),
        });
        cx.notify();
    }

    fn delete_member(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self
            .info
            .as_ref()
            .and_then(|i| i.members.get(self.row))
            .cloned()
        else {
            return;
        };
        let name = self.name.clone();
        self.session.update(cx, |s, cx| {
            s.mutate(
                EnumDelete {
                    name,
                    index: m.index,
                },
                cx,
            )
        });
    }

    fn add_member(&mut self, cx: &mut Context<Self>) {
        let name = self.name.clone();
        let session = self.session.clone();
        let next = self
            .info
            .as_ref()
            .and_then(|i| i.members.iter().map(|m| m.value).max())
            .map_or(0, |v| v + 1);
        let hex = self.hex;
        self.session.update(cx, |s, cx| {
            s.prompt(
                "Add enum member: NAME = value",
                format!("NEW_MEMBER = {}", format_value(next, hex)),
                move |text, cx| {
                    let parsed = text
                        .split_once('=')
                        .and_then(|(n, v)| Some((n.trim().to_owned(), parse_value(v)?)));
                    match parsed {
                        Some((member, value)) if !member.is_empty() => {
                            session.update(cx, |s, cx| {
                                s.mutate(
                                    EnumAdd {
                                        name: name.clone(),
                                        member,
                                        value,
                                    },
                                    cx,
                                )
                            })
                        }
                        _ => session.update(cx, |s, cx| {
                            s.error(format!("'{text}' is not NAME = value"), cx)
                        }),
                    }
                },
                cx,
            )
        });
    }

    fn render_table(
        &self,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let info = self.info.clone();
        let selected = self.row;
        let hex = self.hex;
        let entity = cx.entity();
        let edit = self.edit.as_ref().map(|e| (e.row, e.col, e.input.clone()));
        let list = uniform_list("enum-rows", self.count(), move |range, _w, _cx| {
            let Some(info) = info.as_ref() else {
                return Vec::new();
            };
            range
                .map(|ix| {
                    let m = &info.members[ix];
                    let entity = entity.clone();
                    let cells: [(usize, SharedString); 3] = [
                        (NAME, m.name.clone().into()),
                        (VALUE, format_value(m.value, hex).into()),
                        (COMMENT, m.cmt.clone().into()),
                    ];
                    table::row_shell(("erow", ix), ix == selected, false, &theme)
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

impl Focusable for EnumView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for EnumView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let (title, flags) = match &self.info {
            Some(i) => (
                format!(
                    "enum {}  ·  {} bytes  ·  {} members",
                    i.name,
                    i.width,
                    i.members.len()
                ),
                i.bitfield,
            ),
            None => (self.name.clone(), false),
        };
        let editing = self.edit.is_some();
        div()
            .id("enum-view")
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
                v.begin_edit(r, VALUE, window, cx)
            }))
            .on_action(cx.listener(|v, _: &Comment, window, cx| {
                let r = v.row;
                v.begin_edit(r, COMMENT, window, cx)
            }))
            .on_action(cx.listener(|v, _: &Undefine, _, cx| v.delete_member(cx)))
            .on_action(cx.listener(|v, _: &DeleteMember, _, cx| v.delete_member(cx)))
            .on_action(cx.listener(|v, _: &AddField, _, cx| v.add_member(cx)))
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
                    .child(table::button("add", "Add member…  Ins", &theme).on_click(cx.listener(|v, _, _, cx| v.add_member(cx))))
                    .child(
                        table::button("radix", if self.hex { "Show decimal" } else { "Show hex" }, &theme).on_click(cx.listener(|v, _, _, cx| {
                            v.hex = !v.hex;
                            cx.notify();
                        })),
                    )
                    .child(
                        table::button("flags", if flags { "Bit flags: on" } else { "Bit flags: off" }, &theme).on_click(cx.listener(
                            move |v, _, _, cx| {
                                let name = v.name.clone();
                                v.session.update(cx, |s, cx| s.mutate(EnumSetBitfield { name, on: !flags }, cx));
                            },
                        )),
                    )
                    .child(table::button("types", "Local types", &theme).on_click(cx.listener(|v, _, _, cx| {
                        cx.emit(DocEvent::Open(DocKey::LocalTypes(Some(v.name.clone()))))
                    }))),
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
                    .child("N rename   Y value   ; comment   U / Del delete   Ins add   double-click edits"),
            )
    }
}
