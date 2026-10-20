// Side panel: filterable, virtualised lists of everything in the database.
// Functions and names come from the session cache; the rest load on first view.
use crate::actions::{Confirm, SelectNext, SelectPrev};
use crate::docs::DocKey;
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{Ea, Exports, Imports, SegRow, Segments, Strings, TypeList};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Functions,
    Names,
    Strings,
    Imports,
    Exports,
    Segments,
    Types,
}

/// The activity rail's entries, top to bottom.
pub const TABS: [Tab; 7] = [
    Tab::Functions,
    Tab::Names,
    Tab::Strings,
    Tab::Imports,
    Tab::Exports,
    Tab::Segments,
    Tab::Types,
];

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Functions => "Functions",
            Tab::Names => "Names",
            Tab::Strings => "Strings",
            Tab::Imports => "Imports",
            Tab::Exports => "Exports",
            Tab::Segments => "Segments",
            Tab::Types => "Types",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tab::Functions => "functions",
            Tab::Names => "names",
            Tab::Strings => "strings",
            Tab::Imports => "imports",
            Tab::Exports => "exports",
            Tab::Segments => "segments",
            Tab::Types => "types",
        }
    }

    /// The name stored in settings.
    pub fn key(self) -> &'static str {
        match self {
            Tab::Functions => "functions",
            Tab::Names => "names",
            Tab::Strings => "strings",
            Tab::Imports => "imports",
            Tab::Exports => "exports",
            Tab::Segments => "segments",
            Tab::Types => "types",
        }
    }

    pub fn from_key(key: &str) -> Self {
        TABS.into_iter()
            .find(|t| t.key() == key)
            .unwrap_or(Tab::Functions)
    }
}

#[derive(Clone)]
struct Item {
    label: SharedString,
    detail: SharedString,
    /// Lower-cased label + detail for filtering.
    key: String,
    ea: Option<Ea>,
    badge: Option<(SharedString, u8)>,
    /// Local types: the editor this opens.
    doc: Option<DocKey>,
    /// The mangled name when `label` is its demangled form (tooltip; also filterable).
    mangled: Option<SharedString>,
}

impl Item {
    fn new(
        label: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        ea: Option<Ea>,
    ) -> Self {
        let label = label.into();
        let detail = detail.into();
        let key = format!("{} {}", label, detail).to_lowercase();
        Self {
            label,
            detail,
            key,
            ea,
            badge: None,
            doc: None,
            mangled: None,
        }
    }

    /// Shows `short` (when there is one) and keeps `name` as the tooltip and
    /// the filter key.
    fn demangled(name: String, short: String, detail: String, ea: Ea) -> Self {
        if short.is_empty() || short == name {
            return Self::new(name, detail, Some(ea));
        }
        let mut it = Self::new(short, detail, Some(ea));
        it.key.push(' ');
        it.key.push_str(&name.to_lowercase());
        it.mangled = Some(name.into());
        it
    }
}

pub struct Sidebar {
    session: Entity<Session>,
    tab: Tab,
    filter: Entity<TextInput>,
    items: Rc<Vec<Item>>,
    filtered: Rc<Vec<usize>>,
    selected: usize,
    loaded: Vec<(Tab, Rc<Vec<Item>>)>,
    /// Segment table shown under the list.
    segments: Rc<Vec<SegRow>>,
    loading: bool,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Sidebar {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextInput::new("", "Filter functions", cx).with_icon("search"));
        let s1 = cx.subscribe(&filter, |this, _, _: &InputEvent, cx| this.refilter(cx));
        let s2 = cx.subscribe(&session, |this, _, ev, cx| match ev {
            SessionEvent::ListsReloaded => {
                if matches!(this.tab, Tab::Functions | Tab::Names) {
                    this.rebuild(cx);
                    let at = this.session.read(cx).cursor;
                    this.follow(at, cx);
                }
            }
            SessionEvent::Changed => {
                // Contents of these lists can change with any edit.
                this.loaded
                    .retain(|(t, _)| !matches!(t, Tab::Strings | Tab::Types));
                if matches!(this.tab, Tab::Strings | Tab::Types) {
                    this.rebuild(cx);
                }
            }
            SessionEvent::Navigated { ea } | SessionEvent::CursorMoved { ea, .. } => {
                this.follow(*ea, cx)
            }
            SessionEvent::Opened | SessionEvent::Closed => {
                this.loaded.clear();
                this.rebuild(cx);
                this.load_segments(cx);
            }
            _ => {}
        });
        let mut me = Self {
            session,
            tab: Tab::Functions,
            filter,
            items: Rc::default(),
            filtered: Rc::default(),
            selected: 0,
            loaded: Vec::new(),
            segments: Rc::default(),
            loading: false,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subs: vec![s1, s2],
        };
        me.rebuild(cx);
        me
    }

    /// Selects the function that contains `ea` in the Functions list.
    fn follow(&mut self, ea: Ea, cx: &mut Context<Self>) {
        if self.tab != Tab::Functions {
            return;
        }
        let Some(start) = self.session.read(cx).function_at(ea).map(|f| f.start) else {
            return;
        };
        if let Some(pos) = self
            .filtered
            .iter()
            .position(|&i| self.items[i].ea == Some(start))
            && pos != self.selected
        {
            self.selected = pos;
            self.scroll.scroll_to_item(pos, ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// Switches the list (the activity rail).
    pub fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.set_tab(tab, cx);
    }

    fn load_segments(&mut self, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            self.segments = Rc::default();
            return;
        }
        let fut = self.session.read(cx).call(Segments {});
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = fut.await {
                this.update(cx, |v, cx| {
                    v.segments = Rc::new(rows);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        let f = self.filter.read(cx).focus_handle(cx);
        window.focus(&f, cx);
    }

    /// Windows menu: switch to `tab` and put the cursor in its filter.
    pub fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.set_tab(tab, cx);
        self.focus_filter(window, cx);
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.selected = 0;
        let hint = format!("Filter {}", tab.label().to_lowercase());
        self.filter.update(cx, |f, cx| f.set_placeholder(&hint, cx));
        self.rebuild(cx);
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let s = self.session.read(cx);
        let items: Option<Vec<Item>> = match self.tab {
            Tab::Functions => Some(
                s.functions
                    .iter()
                    .map(|f| {
                        let mut it =
                            Item::new(f.name.clone(), format!("{:X}", f.start), Some(f.start));
                        if f.library {
                            it.badge = Some(("lib".into(), 0));
                        } else if f.thunk {
                            it.badge = Some(("thunk".into(), 1));
                        }
                        it
                    })
                    .collect(),
            ),
            Tab::Names => Some(
                s.names
                    .iter()
                    .map(|n| Item::new(n.name.clone(), format!("{:X}", n.ea), Some(n.ea)))
                    .collect(),
            ),
            tab => self
                .loaded
                .iter()
                .find(|(t, _)| *t == tab)
                .map(|(_, v)| v.as_ref().clone()),
        };
        match items {
            Some(items) => {
                self.items = Rc::new(items);
                self.refilter(cx);
            }
            None => self.load(self.tab, cx),
        }
    }

    fn load(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            self.items = Rc::default();
            self.refilter(cx);
            return;
        }
        self.loading = true;
        self.items = Rc::default();
        self.refilter(cx);
        let s = self.session.read(cx);
        let fut: futures::future::LocalBoxFuture<'static, Result<Vec<Item>, String>> = match tab {
            Tab::Strings => {
                let f = s.call(Strings {});
                Box::pin(async move {
                    f.await.map_err(|e| e.to_string()).map(|v| {
                        v.into_iter()
                            .map(|r| {
                                let mut it = Item::new(
                                    r.text.replace(['\r', '\n'], " "),
                                    format!("{:X}", r.ea),
                                    Some(r.ea),
                                );
                                if r.kind != "C" {
                                    it.badge = Some((r.kind.into(), 2));
                                }
                                it
                            })
                            .collect()
                    })
                })
            }
            Tab::Imports => {
                let f = s.call(Imports {});
                Box::pin(async move {
                    f.await.map_err(|e| e.to_string()).map(|v| {
                        v.into_iter()
                            .map(|r| {
                                let mut it =
                                    Item::demangled(r.name, r.short, format!("{:X}", r.ea), r.ea);
                                it.badge = Some((r.module.to_lowercase().into(), 1));
                                it.key.push(' ');
                                it.key
                                    .push_str(&it.badge.as_ref().unwrap().0.to_lowercase());
                                it
                            })
                            .collect()
                    })
                })
            }
            Tab::Exports => {
                let f = s.call(Exports {});
                Box::pin(async move {
                    f.await.map_err(|e| e.to_string()).map(|v| {
                        v.into_iter()
                            .map(|r| Item::demangled(r.name, r.short, format!("{:X}", r.ea), r.ea))
                            .collect()
                    })
                })
            }
            Tab::Segments => {
                let f = s.call(Segments {});
                Box::pin(async move {
                    f.await.map_err(|e| e.to_string()).map(|v| {
                        v.into_iter()
                            .map(|r| {
                                let mut it = Item::new(
                                    format!("{}  {}", r.name, r.perm),
                                    format!("{:X}-{:X}", r.start, r.end),
                                    Some(r.start),
                                );
                                it.badge = Some((format!("{}·{}", r.class, r.bitness).into(), 1));
                                it
                            })
                            .collect()
                    })
                })
            }
            Tab::Types => {
                let f = s.call(TypeList {});
                Box::pin(async move {
                    f.await.map_err(|e| e.to_string()).map(|v| {
                        v.into_iter()
                            .map(|r| {
                                let size = r.size.map(|s| format!("{s:#x}")).unwrap_or_default();
                                let doc = match r.kind.as_str() {
                                    "struct" | "union" => DocKey::Struct(r.name.clone()),
                                    "enum" => DocKey::Enum(r.name.clone()),
                                    _ => DocKey::LocalTypes(Some(r.name.clone())),
                                };
                                let mut it = Item::new(r.name, size, None);
                                it.key.push(' ');
                                it.key.push_str(&r.kind);
                                it.badge = Some((r.kind.into(), 3));
                                it.doc = Some(doc);
                                it
                            })
                            .collect()
                    })
                })
            }
            Tab::Functions | Tab::Names => return,
        };
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| {
                v.loading = false;
                match result {
                    Ok(items) => {
                        let items = Rc::new(items);
                        v.loaded.retain(|(t, _)| *t != tab);
                        v.loaded.push((tab, items.clone()));
                        if v.tab == tab {
                            v.items = items;
                            v.refilter(cx);
                        }
                    }
                    Err(e) => v.session.update(cx, |s, cx| s.error(e, cx)),
                }
            })
            .ok();
        })
        .detach();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.filter.read(cx).value().trim().to_lowercase();
        let filtered: Vec<usize> = if q.is_empty() {
            (0..self.items.len()).collect()
        } else {
            self.items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.key.contains(&q))
                .map(|(i, _)| i)
                .collect()
        };
        self.filtered = Rc::new(filtered);
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
        cx.notify();
    }

    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(&i) = self.filtered.get(ix) else {
            return;
        };
        self.selected = ix;
        let item = self.items[i].clone();
        if let Some(ea) = item.ea {
            self.session.update(cx, |s, cx| s.navigate(ea, cx));
        } else if let Some(key) = item.doc {
            self.session.update(cx, |_, cx| {
                cx.emit(crate::session::SessionEvent::OpenDoc(key))
            });
        }
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.filtered.is_empty() {
            return;
        }
        let n = self.filtered.len() as isize;
        self.selected = (self.selected as isize + delta).clamp(0, n - 1) as usize;
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let bar = scrollbar::scrollbar(
            "sidebar-scroll",
            Source::list(&self.scroll),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let items = self.items.clone();
        let filtered = self.filtered.clone();
        let selected = self.selected;
        let count = filtered.len();
        let entity = cx.entity();
        let tab = self.tab;
        let list = uniform_list("sidebar-items", count, move |range, _window, _cx| {
            range
                .map(|ix| {
                    let it = &items[filtered[ix]];
                    let entity = entity.clone();
                    let on = ix == selected;
                    div()
                        .id(ix)
                        .w_full()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(hsla(theme.text))
                        .when(on, |d| {
                            d.bg(hsla(theme.accent_soft))
                                .font_weight(gpui::FontWeight::MEDIUM)
                        })
                        .when(!on, |d| d.hover(|d| d.bg(hsla(theme.hover))))
                        .when_some(it.mangled.clone(), |d, m| d.tooltip(ui::tip(m, None)))
                        .on_click(move |ev, _, cx| {
                            entity.update(cx, |v, cx| {
                                // Types open an editor: that takes a double click.
                                if v.tab == Tab::Types && ev.click_count() < 2 {
                                    v.selected = ix;
                                    cx.notify();
                                } else {
                                    v.activate(ix, cx)
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
                                .child(it.label.clone()),
                        )
                        .when_some(it.badge.clone(), |d, (b, _)| {
                            d.child(ui::badge(b, ui::BadgeKind::Neutral, &theme))
                        })
                        .when(tab != Tab::Types || !it.detail.is_empty(), |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .font_family(code_font_name())
                                    .text_size(px(11.))
                                    .text_color(hsla(theme.muted))
                                    .child(it.detail.clone()),
                            )
                        })
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();

        let segments = (tab != Tab::Segments && !self.segments.is_empty())
            .then(|| self.render_segments(&theme));
        div()
            .key_context("List")
            .track_focus(&self.focus)
            .on_action(cx.listener(|v, _: &SelectPrev, _, cx| v.step(-1, cx)))
            .on_action(cx.listener(|v, _: &SelectNext, _, cx| v.step(1, cx)))
            .on_action(cx.listener(|v, _: &Confirm, _, cx| {
                let s = v.selected;
                v.activate(s, cx)
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface2))
            .text_size(px(12.))
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(12.))
                    .pr(px(8.))
                    .child(ui::section_label(self.tab.label(), &theme))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(hsla(theme.muted))
                            .child(if self.loading {
                                "…".to_owned()
                            } else {
                                self.filtered.len().to_string()
                            }),
                    )
                    .child(
                        ui::icon_button("sidebar-filter", "filter", 24., false, true, &theme)
                            .on_click(cx.listener(|v, _, window, cx| v.focus_filter(window, cx))),
                    ),
            )
            .child(div().px(px(8.)).pb(px(8.)).child(self.filter.clone()))
            .child(scrollbar::area(list, bar))
            .when_some(segments, |d, s| d.child(s))
    }
}

impl Sidebar {
    /// A compact segment table under the list, as in the design's main window.
    fn render_segments(&self, theme: &Theme) -> gpui::Div {
        let t = *theme;
        let rows: Vec<_> = self.segments.iter().take(8).collect();
        div()
            .flex_none()
            .border_t_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .child(ui::section_label("Segments", &t))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(self.segments.len().to_string()),
                    ),
            )
            .child(div().pb(px(4.)).children(rows.into_iter().map(|r| {
                div()
                    .h(px(22.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(11.))
                    .child(
                        div()
                            .w(px(60.))
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .child(r.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_family(code_font_name())
                            .text_color(hsla(t.muted))
                            .child(format!("{:X}", r.start)),
                    )
                    .child(
                        div()
                            .w(px(40.))
                            .flex_none()
                            .text_color(hsla(t.muted))
                            .child(r.class.clone()),
                    )
                    .child(
                        div()
                            .font_family(code_font_name())
                            .text_color(hsla(t.muted))
                            .child(r.perm.clone()),
                    )
            })))
    }
}
