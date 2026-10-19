// The local types editor: a filterable list of every type on the left and the
// selected type's C declaration in a code editor on the right. Applying the
// text hands it to IDA's type parser; its errors come back as markers on the
// offending lines.
use crate::actions::{ApplyDecl, Confirm, SelectNext, SelectPrev};
use crate::docs::{DocEvent, DocKey};
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::table;
use crate::theme::{Theme, code_font_name, code_font_size, code_line_height, hsla};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{
    ApplyReply, ClientError, TypeApply, TypeBrief, TypeCreate, TypeDecl, TypeDelete, TypeList,
};
use std::rc::Rc;
use zui_editor::{CHighlighter, Editor, EditorEvent, ErrorMarker, Pos};

/// A destructive step that needs a second press to go through.
#[derive(Clone, PartialEq, Eq)]
enum Armed {
    /// Switching away from edits that were not applied.
    Discard(String),
    /// Deleting a type other types depend on.
    Delete(String),
}

pub struct TypesView {
    session: Entity<Session>,
    filter: Entity<TextInput>,
    all: Rc<Vec<TypeBrief>>,
    shown: Rc<Vec<usize>>,
    /// The type whose declaration the editor holds (`None`: a new text).
    current: Option<String>,
    /// The highlighted list row.
    row: usize,
    editor: Entity<Editor>,
    /// The editor's text as last fetched or applied.
    loaded: String,
    importing: bool,
    /// Parser messages with the 0-based line each belongs to.
    messages: Vec<(Option<usize>, SharedString)>,
    status: Option<(SharedString, bool)>,
    armed: Option<Armed>,
    style: (&'static str, SharedString, u32),
    scroll: UniformListScrollHandle,
    loading: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DocEvent> for TypesView {}

impl TypesView {
    pub fn new(
        session: Entity<Session>,
        select: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = Theme::get(cx);
        let filter = cx.new(|cx| TextInput::new("", "Filter types", cx));
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
            cx.subscribe(&filter, |this, _, _: &InputEvent, cx| this.refilter(cx)),
            cx.subscribe(&editor, |this, _, ev: &EditorEvent, cx| match ev {
                EditorEvent::Changed => {
                    this.armed = None;
                    cx.notify();
                }
                EditorEvent::Submit => this.apply(cx),
                _ => {}
            }),
            cx.subscribe(&session, |this, _, ev, cx| match ev {
                SessionEvent::Changed => this.reload(false, cx),
                SessionEvent::Opened | SessionEvent::Closed => {
                    this.all = Rc::default();
                    this.shown = Rc::default();
                    cx.notify();
                }
                _ => {}
            }),
        ];
        let focus = filter.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        let mut me = Self {
            session,
            filter,
            all: Rc::default(),
            shown: Rc::default(),
            current: select.clone(),
            row: 0,
            editor,
            loaded: String::new(),
            importing: false,
            messages: Vec::new(),
            status: None,
            armed: None,
            style: (theme.name, code_font_name(), code_font_size().to_bits()),
            scroll: UniformListScrollHandle::new(),
            loading: false,
            _subs: subs,
        };
        me.reload(true, cx);
        me
    }

    /// Shows `name` in the editor (from the sidebar or the workspace).
    pub fn select_type(&mut self, name: String, cx: &mut Context<Self>) {
        self.load_decl(name, cx);
    }

    fn dirty(&self, cx: &App) -> bool {
        self.editor.read(cx).text() != self.loaded
    }

    /// Refetches the type list; `force` also reloads the open declaration.
    fn reload(&mut self, force: bool, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        self.loading = true;
        let fut = self.session.read(cx).call(TypeList {});
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| {
                v.loading = false;
                match result {
                    Ok(list) => {
                        v.all = Rc::new(list);
                        v.refilter(cx);
                        match v.current.clone() {
                            Some(name) if v.all.iter().any(|t| t.name == name) => {
                                if force || !v.dirty(cx) {
                                    v.load_decl(name, cx);
                                }
                            }
                            Some(_) if !v.dirty(cx) => {
                                // The type is gone (deleted or undone).
                                v.current = None;
                                v.set_editor_text("", cx);
                            }
                            None if !v.importing && !v.dirty(cx) => {
                                if let Some(first) = v.all.first().map(|t| t.name.clone()) {
                                    v.load_decl(first, cx);
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(e) => v.session.update(cx, |s, cx| s.error(e.to_string(), cx)),
                }
            })
            .ok();
        })
        .detach();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.filter.read(cx).value().trim().to_lowercase();
        let shown: Vec<usize> = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                q.is_empty() || t.name.to_lowercase().contains(&q) || t.kind.contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        self.shown = Rc::new(shown);
        self.row = self
            .current
            .as_ref()
            .and_then(|c| self.shown.iter().position(|&i| &self.all[i].name == c))
            .unwrap_or(self.row.min(self.shown.len().saturating_sub(1)));
        cx.notify();
    }

    fn set_editor_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.loaded = text.to_owned();
        self.editor.update(cx, |e, cx| e.set_text(text, cx));
        self.messages.clear();
        cx.notify();
    }

    fn load_decl(&mut self, name: String, cx: &mut Context<Self>) {
        let fut = self.session.read(cx).call(TypeDecl { name: name.clone() });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| match result {
                Ok(d) => {
                    v.current = Some(name.clone());
                    v.importing = false;
                    v.armed = None;
                    v.status = None;
                    v.set_editor_text(&d.decl, cx);
                    if let Some(i) = v.shown.iter().position(|&i| v.all[i].name == name) {
                        v.row = i;
                        v.scroll.scroll_to_item(i, ScrollStrategy::Nearest);
                    }
                    cx.emit(DocEvent::Title(SharedString::from("Local types")));
                }
                Err(e) => v.fail(e, cx),
            })
            .ok();
        })
        .detach();
    }

    fn fail(&mut self, e: ClientError, cx: &mut Context<Self>) {
        self.status = Some((e.to_string().into(), true));
        cx.notify();
    }

    /// A list click or arrow key: show that type, unless that would throw
    /// away edits the user has not applied (the second press discards).
    fn pick(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&i) = self.shown.get(ix) else { return };
        let name = self.all[i].name.clone();
        self.row = ix;
        if self.dirty(cx)
            && self.current.as_ref() != Some(&name)
            && self.armed.as_ref() != Some(&Armed::Discard(name.clone()))
        {
            self.armed = Some(Armed::Discard(name));
            self.status = Some(("Unsaved changes in the editor: apply them with Ctrl+S, or pick the type again to discard them".into(), true));
            cx.notify();
            return;
        }
        self.load_decl(name, cx);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.shown.is_empty() {
            return;
        }
        let ix = (self.row as isize + delta).clamp(0, self.shown.len() as isize - 1) as usize;
        self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        self.pick(ix, cx);
    }

    fn open_editor(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&i) = self.shown.get(ix) else { return };
        let t = &self.all[i];
        let key = match t.kind.as_str() {
            "struct" | "union" => DocKey::Struct(t.name.clone()),
            "enum" => DocKey::Enum(t.name.clone()),
            _ => return,
        };
        cx.emit(DocEvent::Open(key));
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let decl = self.editor.read(cx).text();
        if decl.trim().is_empty() {
            self.status = Some(("Nothing to apply".into(), true));
            cx.notify();
            return;
        }
        let fut = self.session.read(cx).call(TypeApply { decl: decl.clone() });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| match result {
                Ok(reply) => v.applied(decl, reply, cx),
                Err(e) => v.fail(e, cx),
            })
            .ok();
        })
        .detach();
    }

    fn applied(&mut self, decl: String, reply: ApplyReply, cx: &mut Context<Self>) {
        let errors = reply.errors(&decl);
        self.messages = errors
            .iter()
            .map(|e| (Some(e.line), e.message.clone().into()))
            .collect();
        let markers: Vec<ErrorMarker> = errors
            .iter()
            .map(|e| ErrorMarker::new(e.line, e.line, e.message.clone()))
            .collect();
        // Statements without errors are stored even when others failed.
        let stored = reply.applied || !reply.names.is_empty();
        if reply.errors > 0 {
            self.editor.update(cx, |e, cx| e.set_errors(markers, cx));
            let first = errors.first().map(|e| e.line + 1).unwrap_or(1);
            self.status = Some((
                format!(
                    "{} error{} from the type parser; the first is on line {first}",
                    reply.errors,
                    if reply.errors == 1 { "" } else { "s" }
                )
                .into(),
                true,
            ));
        } else {
            let what = match reply.names.as_slice() {
                [] => "Applied (existing types updated)".to_owned(),
                [one] => format!("Applied {one}"),
                many => format!("Applied {} types", many.len()),
            };
            self.status = Some((what.into(), false));
            self.loaded = decl;
            self.importing = false;
            self.armed = None;
            if let Some(name) = reply.names.first() {
                self.current = Some(name.clone());
            }
        }
        if stored {
            self.session.update(cx, |s, cx| s.changed(cx));
        }
        cx.notify();
    }

    fn new_type(&mut self, cx: &mut Context<Self>) {
        let me = cx.entity();
        self.session.update(cx, |s, cx| {
            s.prompt(
                "New type: struct, union or enum, then the name",
                "struct NewType",
                move |text, cx| me.update(cx, |v, cx| v.create(&text, cx)),
                cx,
            )
        });
    }

    fn create(&mut self, text: &str, cx: &mut Context<Self>) {
        let mut words = text.split_whitespace();
        let (kind, name) = match (words.next(), words.next()) {
            (Some(k @ ("struct" | "union" | "enum")), Some(n)) => (k, n.to_owned()),
            (Some(n), None) => ("struct", n.to_owned()),
            _ => {
                self.status = Some((
                    "Write the kind and the name, e.g. struct Point".into(),
                    true,
                ));
                cx.notify();
                return;
            }
        };
        let fut = self.session.read(cx).call(TypeCreate {
            name: name.clone(),
            kind: kind.to_owned(),
        });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| match result {
                Ok(()) => {
                    v.current = Some(name.clone());
                    v.status = Some((format!("Created {name}").into(), false));
                    v.session.update(cx, |s, cx| s.changed(cx));
                }
                Err(e) => v.fail(e, cx),
            })
            .ok();
        })
        .detach();
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.current.clone() else {
            return;
        };
        let force = self.armed == Some(Armed::Delete(name.clone()));
        let fut = self.session.read(cx).call(TypeDelete {
            name: name.clone(),
            force,
        });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| match result {
                Ok(r) if r.deleted => {
                    v.armed = None;
                    v.current = None;
                    v.status = Some((format!("Deleted {name} (Ctrl+Z undoes)").into(), false));
                    v.set_editor_text("", cx);
                    v.session.update(cx, |s, cx| s.changed(cx));
                }
                Ok(r) => {
                    v.armed = Some(Armed::Delete(name.clone()));
                    v.status = Some((
                        format!(
                            "{name} is used by {}. Press Delete again to delete it anyway",
                            r.dependents.join(", ")
                        )
                        .into(),
                        true,
                    ));
                    cx.notify();
                }
                Err(e) => v.fail(e, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Starts an empty text for pasting a header.
    fn import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty(cx) && self.armed != Some(Armed::Discard(String::new())) {
            self.armed = Some(Armed::Discard(String::new()));
            self.status = Some((
                "Unsaved changes in the editor: press Import again to discard them".into(),
                true,
            ));
            cx.notify();
            return;
        }
        self.current = None;
        self.importing = true;
        self.armed = None;
        self.status = None;
        self.set_editor_text("", cx);
        let focus = self.editor.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Keeps the editor in step with the app theme and code font.
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

    fn render_list(
        &self,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let all = self.all.clone();
        let shown = self.shown.clone();
        let row = self.row;
        let entity = cx.entity();
        let bar = scrollbar::scrollbar(
            "types-scroll",
            Source::list(&self.scroll),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let list = uniform_list("types-items", shown.len(), move |range, _window, _cx| {
            range
                .map(|ix| {
                    let t = &all[shown[ix]];
                    let entity = entity.clone();
                    let kind_color = match t.kind.as_str() {
                        "struct" | "union" => hsla(theme.syntax.kw),
                        "enum" => hsla(theme.syntax.num),
                        "typedef" => hsla(theme.syntax.imp_name),
                        _ => hsla(theme.muted),
                    };
                    div()
                        .id(ix)
                        .w_full()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .cursor_pointer()
                        .when(ix == row, |d| d.bg(hsla(theme.accent_soft)))
                        .hover(|d| d.bg(hsla(theme.hover)))
                        .on_click(move |ev, _, cx| {
                            entity.update(cx, |v, cx| {
                                v.pick(ix, cx);
                                if ev.click_count() >= 2 {
                                    v.open_editor(ix, cx);
                                }
                            })
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(t.name.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(10.))
                                .px(px(5.))
                                .rounded(px(3.))
                                .bg(hsla(theme.raised))
                                .text_color(kind_color)
                                .child(t.kind.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(px(44.))
                                .flex()
                                .justify_end()
                                .font_family(code_font_name())
                                .text_size(px(11.))
                                .text_color(hsla(theme.faint))
                                .child(t.size.map(|s| format!("{s:#x}")).unwrap_or_default()),
                        )
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();
        div()
            .key_context("List")
            .on_action(cx.listener(|v, _: &SelectPrev, _, cx| v.step(-1, cx)))
            .on_action(cx.listener(|v, _: &SelectNext, _, cx| v.step(1, cx)))
            .on_action(cx.listener(|v, _: &Confirm, _, cx| {
                let ix = v.row;
                v.open_editor(ix, cx);
            }))
            .w(px(300.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface2))
            .border_r_1()
            .border_color(hsla(theme.border))
            .text_size(px(12.5))
            .child(div().p(px(8.)).child(self.filter.clone()))
            .child(
                div()
                    .px(px(12.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .text_color(hsla(theme.faint))
                    .child(if self.loading {
                        "Loading…".to_owned()
                    } else {
                        format!("{} of {} types", self.shown.len(), self.all.len())
                    }),
            )
            .child(scrollbar::area(list, bar))
    }

    fn render_messages(
        &self,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let mh = scrollbar::handle("type-msg-h", window, cx);
        let mbar = scrollbar::scrollbar(
            "type-msg-scroll",
            Source::handle(&mh),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let status = self.status.clone();
        let messages = self.messages.clone();
        let editor = self.editor.clone();
        div()
            .relative()
            .flex_none()
            .max_h(px(150.))
            .border_t_1()
            .border_color(hsla(theme.border))
            .bg(hsla(theme.surface2))
            .child(
                div()
                    .id("type-messages")
                    .max_h(px(150.))
                    .overflow_y_scroll()
                    .track_scroll(&mh)
                    .text_size(px(12.))
                    .when_some(status, |d, (text, err)| {
                        d.child(
                            div()
                                .px(px(12.))
                                .py(px(5.))
                                .text_color(if err {
                                    hsla(theme.danger)
                                } else {
                                    hsla(theme.success)
                                })
                                .child(text),
                        )
                    })
                    .children(
                        messages
                            .into_iter()
                            .enumerate()
                            .map(move |(i, (line, text))| {
                                let editor = editor.clone();
                                div()
                                    .id(("msg", i))
                                    .px(px(12.))
                                    .py(px(2.))
                                    .flex()
                                    .gap(px(10.))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(hsla(theme.hover)))
                                    .when_some(line, |d, line| {
                                        d.on_click(move |_, window, cx| {
                                            editor.update(cx, |e, cx| {
                                                e.set_cursor(Pos { line, col: 0 }, cx);
                                                e.focus(window, cx);
                                            })
                                        })
                                        .child(
                                            div()
                                                .flex_none()
                                                .w(px(60.))
                                                .font_family(code_font_name())
                                                .text_color(hsla(theme.muted))
                                                .child(format!("line {}", line + 1)),
                                        )
                                    })
                                    .child(div().text_color(hsla(theme.danger)).child(text))
                            }),
                    ),
            )
            .child(mbar)
    }
}

impl Focusable for TypesView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.read(cx).focus_handle(cx)
    }
}

impl Render for TypesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_style(cx);
        let theme = Theme::get(cx);
        let editor_bar = scrollbar::scrollbar(
            "decl-scroll",
            scrollbar::editor_source(&self.editor),
            Axes::BOTH,
            &theme,
            window,
            cx,
        );
        let current = self.current.clone();
        let brief = current
            .as_ref()
            .and_then(|c| self.all.iter().find(|t| &t.name == c))
            .cloned();
        let dirty = self.dirty(cx);
        // The header says what the editor holds: the selected type while the
        // text is untouched, otherwise what the typed text declares.
        let declared = if dirty {
            crate::decl_names::declared_names(&self.editor.read(cx).text())
        } else {
            Vec::new()
        };
        let title: SharedString = match (dirty, &current, self.importing) {
            (true, _, _) => match declared.as_slice() {
                [] => "Unsaved declaration".into(),
                [one] => one.clone().into(),
                [first, rest @ ..] => format!("{first} +{} more", rest.len()).into(),
            },
            (false, Some(name), _) => name.clone().into(),
            (false, None, true) => "Import C declarations".into(),
            (false, None, false) => "Select a type".into(),
        };
        let edit_note = declared.first().map(|n| {
            if current.as_ref() == Some(n) {
                "modified"
            } else {
                "new declaration"
            }
        });
        let brief = if dirty { None } else { brief };
        let editable = brief
            .as_ref()
            .is_some_and(|t| matches!(t.kind.as_str(), "struct" | "union" | "enum"));
        let idx = brief
            .as_ref()
            .and_then(|b| self.shown.iter().position(|&i| self.all[i].name == b.name));
        let _ = window;
        div()
            .key_context("TypeEditor")
            .on_action(cx.listener(|v, _: &ApplyDecl, _, cx| v.apply(cx)))
            .size_full()
            .flex()
            .bg(hsla(theme.surface))
            .child(self.render_list(theme, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
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
                            .child(
                                div()
                                    .font_family(code_font_name())
                                    .text_size(px(14.))
                                    .text_color(hsla(theme.text))
                                    .child(title),
                            )
                            .when_some(brief.clone(), |d, b| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(hsla(theme.muted))
                                        .child(format!(
                                            "{}{}",
                                            b.kind,
                                            b.size
                                                .map(|s| format!(" · {s:#x} bytes"))
                                                .unwrap_or_default()
                                        )),
                                )
                            })
                            .when(dirty, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(hsla(theme.warning))
                                        .child(format!("● {}", edit_note.unwrap_or("modified"))),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                table::primary_button("apply", "Apply  Ctrl+S", &theme)
                                    .on_click(cx.listener(|v, _, _, cx| v.apply(cx))),
                            )
                            .child(
                                table::button("new", "New type…", &theme)
                                    .on_click(cx.listener(|v, _, _, cx| v.new_type(cx))),
                            )
                            .child(
                                table::button("import", "Import C header…", &theme)
                                    .on_click(cx.listener(|v, _, window, cx| v.import(window, cx))),
                            )
                            .when(editable, |d| {
                                d.child(table::button("open", "Open editor", &theme).on_click(
                                    cx.listener(move |v, _, _, cx| {
                                        if let Some(ix) = idx {
                                            v.open_editor(ix, cx);
                                        }
                                    }),
                                ))
                            })
                            .when(current.is_some(), |d| {
                                d.child(
                                    table::button("delete", "Delete", &theme)
                                        .on_click(cx.listener(|v, _, _, cx| v.delete(cx))),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .relative()
                            .child(self.editor.clone())
                            .child(editor_bar),
                    )
                    .child(self.render_messages(theme, window, cx)),
            )
    }
}
