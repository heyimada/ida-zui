// Side panel: filterable, virtualised lists of everything in the database.
// Functions and names come from the session cache; the rest load on first view.
use crate::actions::{Confirm, SelectNext, SelectPrev};
use crate::docs::DocKey;
use crate::input::{InputEvent, TextInput};
use crate::list_table::{self, SortKey, TableState};
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::settings;
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, ScrollStrategy, SharedString,
    Subscription, UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
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

/// A column of a table list: title, default width, whether it takes the
/// free space, and whether its text is numeric (monospace, right-aligned).
struct ColSpec {
    title: &'static str,
    width: f32,
    grow: bool,
    numeric: bool,
}

const fn col(title: &'static str, width: f32, grow: bool, numeric: bool) -> ColSpec {
    ColSpec {
        title,
        width,
        grow,
        numeric,
    }
}

const FUNCTION_COLS: [ColSpec; 5] = [
    col("Name", 130., true, false),
    col("Segment", 60., false, false),
    col("Start", 78., false, true),
    col("Length", 52., false, true),
    col("Flags", 56., false, false),
];
const NAME_COLS: [ColSpec; 3] = [
    col("Address", 78., false, true),
    col("Name", 160., true, false),
    col("Type", 56., false, false),
];
const STRING_COLS: [ColSpec; 4] = [
    col("Address", 78., false, true),
    col("Length", 56., false, true),
    col("Type", 52., false, false),
    col("String", 160., true, false),
];
const IMPORT_COLS: [ColSpec; 4] = [
    col("Address", 78., false, true),
    col("Ordinal", 58., false, true),
    col("Name", 160., true, false),
    col("Library", 100., false, false),
];
const EXPORT_COLS: [ColSpec; 3] = [
    col("Name", 160., true, false),
    col("Address", 78., false, true),
    col("Ordinal", 58., false, true),
];

/// The columns of a table list; `None` for the lists that stay plain rows.
fn columns(tab: Tab) -> Option<&'static [ColSpec]> {
    match tab {
        Tab::Functions => Some(&FUNCTION_COLS),
        Tab::Names => Some(&NAME_COLS),
        Tab::Strings => Some(&STRING_COLS),
        Tab::Imports => Some(&IMPORT_COLS),
        Tab::Exports => Some(&EXPORT_COLS),
        Tab::Segments | Tab::Types => None,
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
    /// Table lists: the text of each column, what each sorts by, and the
    /// flag badges of the Flags column.
    cells: Vec<SharedString>,
    sort: Vec<SortKey>,
    flags: Vec<&'static str>,
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
            cells: Vec::new(),
            sort: Vec::new(),
            flags: Vec::new(),
        }
    }

    /// A row of a table list: the cells, their sort keys, the name that names
    /// the row (menus, copy) and the text the filter searches.
    fn table(
        name: String,
        ea: Ea,
        cells: Vec<String>,
        sort: Vec<SortKey>,
        mangled: Option<String>,
    ) -> Self {
        let mut key = cells.join(" ").to_lowercase();
        if let Some(m) = &mangled {
            key.push(' ');
            key.push_str(&m.to_lowercase());
        }
        Self {
            label: name.into(),
            detail: SharedString::default(),
            key,
            ea: Some(ea),
            badge: None,
            doc: None,
            mangled: mangled.map(Into::into),
            cells: cells.into_iter().map(Into::into).collect(),
            sort,
            flags: Vec::new(),
        }
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
    /// Horizontal scroll of a table wider than the panel.
    hscroll: ScrollHandle,
    /// Sort and widths of the table showing.
    table: TableState,
    /// A column edge being dragged: (column, last x).
    resizing: Option<(usize, f32)>,
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
            hscroll: ScrollHandle::new(),
            table: TableState::default(),
            resizing: None,
            focus: cx.focus_handle(),
            _subs: vec![s1, s2],
        };
        me.table = me.saved_table(cx);
        me.rebuild(cx);
        me
    }

    /// The sort and widths saved for the current list.
    fn saved_table(&self, cx: &App) -> TableState {
        settings::get(cx)
            .side_tables
            .get(self.tab.key())
            .cloned()
            .unwrap_or_default()
    }

    fn save_table(&self, cx: &mut Context<Self>) {
        let (key, state) = (self.tab.key().to_owned(), self.table.clone());
        settings::update(cx, move |s| {
            s.side_tables.insert(key, state);
        });
    }

    /// The column widths to draw.
    fn widths(&self, cols: &[ColSpec]) -> Vec<f32> {
        let defaults: Vec<f32> = cols.iter().map(|c| c.width).collect();
        self.table.widths_or(&defaults)
    }

    /// A click on a header: sort by that column, or flip the direction.
    fn click_header(&mut self, col: usize, cx: &mut Context<Self>) {
        self.table.click(col);
        self.save_table(cx);
        self.refilter(cx);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    /// Dragging a column edge.
    fn drag_edge(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some((col, last)) = self.resizing else {
            return;
        };
        let Some(cols) = columns(self.tab) else {
            return;
        };
        let mut widths = self.widths(cols);
        if let Some(w) = widths.get_mut(col) {
            *w = list_table::resized(*w, x - last);
        }
        self.table.widths = widths;
        self.resizing = Some((col, x));
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.resizing.take().is_some() {
            self.save_table(cx);
        }
    }

    /// The segment that holds `ea`.
    fn segment_at(&self, ea: Ea) -> Option<&SegRow> {
        self.segments.iter().find(|s| ea >= s.start && ea < s.end)
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
                    // The Segment and Type columns of Functions / Names read them.
                    if matches!(v.tab, Tab::Functions | Tab::Names) {
                        v.rebuild(cx);
                    }
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
        self.table = self.saved_table(cx);
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
                        let seg = self
                            .segment_at(f.start)
                            .map(|s| s.name.clone())
                            .unwrap_or_default();
                        let len = f.end.saturating_sub(f.start);
                        let flags: Vec<&'static str> = [(f.library, "lib"), (f.thunk, "thunk")]
                            .into_iter()
                            .filter(|(on, _)| *on)
                            .map(|(_, n)| n)
                            .collect();
                        let mut it = Item::table(
                            f.name.clone(),
                            f.start,
                            vec![
                                f.name.clone(),
                                seg.clone(),
                                format!("{:X}", f.start),
                                format!("{len:X}"),
                                flags.join(" "),
                            ],
                            vec![
                                SortKey::text(&f.name),
                                SortKey::text(&seg),
                                SortKey::Num(f.start),
                                SortKey::Num(len),
                                SortKey::text(&flags.join(" ")),
                            ],
                            None,
                        );
                        it.flags = flags;
                        it
                    })
                    .collect(),
            ),
            Tab::Names => Some(
                s.names
                    .iter()
                    .map(|n| {
                        let kind = if s.function_at(n.ea).is_some_and(|f| f.start == n.ea) {
                            "func".to_owned()
                        } else {
                            self.segment_at(n.ea)
                                .map(|g| g.class.to_lowercase())
                                .unwrap_or_default()
                        };
                        Item::table(
                            n.name.clone(),
                            n.ea,
                            vec![format!("{:X}", n.ea), n.name.clone(), kind.clone()],
                            vec![
                                SortKey::Num(n.ea),
                                SortKey::text(&n.name),
                                SortKey::text(&kind),
                            ],
                            None,
                        )
                    })
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
                                let text = r.text.replace(['\r', '\n'], " ");
                                Item::table(
                                    text.clone(),
                                    r.ea,
                                    vec![
                                        format!("{:X}", r.ea),
                                        r.len.to_string(),
                                        r.kind.clone(),
                                        text.clone(),
                                    ],
                                    vec![
                                        SortKey::Num(r.ea),
                                        SortKey::Num(u64::from(r.len)),
                                        SortKey::text(&r.kind),
                                        SortKey::text(&text),
                                    ],
                                    None,
                                )
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
                                let shown = if r.short.is_empty() {
                                    r.name.clone()
                                } else {
                                    r.short.clone()
                                };
                                let mangled = (shown != r.name).then(|| r.name.clone());
                                let ordinal = if r.ordinal == 0 {
                                    String::new()
                                } else {
                                    r.ordinal.to_string()
                                };
                                Item::table(
                                    shown.clone(),
                                    r.ea,
                                    vec![
                                        format!("{:X}", r.ea),
                                        ordinal,
                                        shown.clone(),
                                        r.module.clone(),
                                    ],
                                    vec![
                                        SortKey::Num(r.ea),
                                        SortKey::Num(u64::from(r.ordinal)),
                                        SortKey::text(&shown),
                                        SortKey::text(&r.module),
                                    ],
                                    mangled,
                                )
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
                            .map(|r| {
                                let shown = if r.short.is_empty() {
                                    r.name.clone()
                                } else {
                                    r.short.clone()
                                };
                                let mangled = (shown != r.name).then(|| r.name.clone());
                                Item::table(
                                    shown.clone(),
                                    r.ea,
                                    vec![
                                        shown.clone(),
                                        format!("{:X}", r.ea),
                                        r.ordinal.to_string(),
                                    ],
                                    vec![
                                        SortKey::text(&shown),
                                        SortKey::Num(r.ea),
                                        SortKey::Num(r.ordinal),
                                    ],
                                    mangled,
                                )
                            })
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
        let mut filtered = filtered;
        if let Some(col) = self.table.sort
            && columns(self.tab).is_some_and(|c| col < c.len())
        {
            let keys: Vec<SortKey> = self
                .items
                .iter()
                .map(|it| it.sort.get(col).cloned().unwrap_or(SortKey::Num(0)))
                .collect();
            list_table::sort_order(&keys, &mut filtered, !self.table.descending);
        }
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

    /// Right click (or the Menu key) on row `ix`.
    fn context_menu(&mut self, ix: usize, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        let Some(&i) = self.filtered.get(ix) else {
            return;
        };
        self.selected = ix;
        let item = self.items[i].clone();
        let entries = if let Some(ea) = item.ea {
            crate::context_menu::address_row(&self.session, ea, Some(item.label.to_string()), cx)
        } else if let Some(key) = item.doc {
            let session = self.session.clone();
            vec![crate::menubar::run_entry("Open", "", move |_, cx| {
                session.update(cx, |_, cx| {
                    cx.emit(crate::session::SessionEvent::OpenDoc(key.clone()))
                })
            })]
        } else {
            return;
        };
        crate::context_menu::show(
            &self.session,
            pos,
            item.label.to_string(),
            entries,
            false,
            cx,
        );
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
        let cols = columns(tab);
        let widths = cols.map(|c| self.widths(c)).unwrap_or_default();
        let total_w: f32 = widths.iter().sum();
        let rows_widths = widths.clone();
        let list = uniform_list("sidebar-items", count, move |range, _window, _cx| {
            range
                .map(|ix| {
                    let it = &items[filtered[ix]];
                    let entity = entity.clone();
                    let on = ix == selected;
                    let row = div()
                        .id(ix)
                        .w_full()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .when(cols.is_none(), |d| d.gap(px(8.)).px(px(12.)))
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(hsla(theme.text))
                        .when(on, |d| {
                            d.bg(hsla(theme.accent_soft))
                                .font_weight(gpui::FontWeight::MEDIUM)
                        })
                        .when(!on, |d| d.hover(|d| d.bg(hsla(theme.hover))))
                        .when_some(it.mangled.clone(), |d, m| d.tooltip(ui::tip(m, None)))
                        .on_mouse_down(gpui::MouseButton::Right, {
                            let entity = entity.clone();
                            move |ev, _, cx| {
                                entity.update(cx, |v, cx| v.context_menu(ix, ev.position, cx))
                            }
                        })
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
                        });
                    match cols {
                        Some(cols) => row.children(cols.iter().enumerate().map(|(c, spec)| {
                            let text = it.cells.get(c).cloned().unwrap_or_default();
                            let w = rows_widths.get(c).copied().unwrap_or(spec.width);
                            table_cell(spec, w, &theme)
                                .when(spec.numeric, |d| {
                                    d.font_family(code_font_name())
                                        .text_size(px(11.))
                                        .text_color(hsla(theme.muted))
                                })
                                .when(spec.title == "Flags", |d| {
                                    d.children(
                                        it.flags
                                            .iter()
                                            .map(|f| ui::badge(*f, ui::BadgeKind::Neutral, &theme)),
                                    )
                                })
                                .when(spec.title != "Flags", |d| d.child(text))
                        })),
                        None => row
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
                            .when(!it.detail.is_empty(), |d| {
                                d.child(
                                    div()
                                        .flex_none()
                                        .font_family(code_font_name())
                                        .text_size(px(11.))
                                        .text_color(hsla(theme.muted))
                                        .child(it.detail.clone()),
                                )
                            }),
                    }
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();

        let header = cols.map(|cols| self.render_table_header(cols, &widths, &theme, cx));
        let body = scrollbar::area(list, bar);
        let table: gpui::AnyElement = match header {
            Some(header) => {
                let hbar = scrollbar::scrollbar(
                    "sidebar-hscroll",
                    Source::handle(&self.hscroll),
                    Axes::BOTH,
                    &theme,
                    window,
                    cx,
                );
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("sidebar-table-x")
                            .size_full()
                            .overflow_x_scroll()
                            .track_scroll(&self.hscroll)
                            .child(
                                div()
                                    .min_w(px(total_w))
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .child(header)
                                    .child(body),
                            ),
                    )
                    .child(hbar)
                    .into_any_element()
            }
            None => body.into_any_element(),
        };

        let segments = (tab != Tab::Segments && !self.segments.is_empty())
            .then(|| self.render_segments(&theme, cx));
        div()
            .key_context("List")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|v, _: &crate::actions::ShowContextMenu, window, cx| {
                    let pos = window.mouse_position();
                    v.context_menu(v.selected, pos, cx)
                }),
            )
            .on_action(cx.listener(|v, _: &SelectPrev, _, cx| v.step(-1, cx)))
            .on_action(cx.listener(|v, _: &SelectNext, _, cx| v.step(1, cx)))
            .on_action(cx.listener(|v, _: &Confirm, _, cx| {
                let s = v.selected;
                v.activate(s, cx)
            }))
            .on_mouse_move(cx.listener(|v, ev: &gpui::MouseMoveEvent, _, cx| {
                if v.resizing.is_some() {
                    if ev.pressed_button == Some(gpui::MouseButton::Left) {
                        v.drag_edge(f32::from(ev.position.x), cx);
                    } else {
                        v.end_drag(cx);
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|v, _, _, cx| v.end_drag(cx)),
            )
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
            .child(table)
            .when_some(segments, |d, s| d.child(s))
    }
}

/// One cell of a table row or header: the column's width (the growing column
/// takes what is left, never less than its width), clipped with an ellipsis.
fn table_cell(spec: &ColSpec, width: f32, _theme: &Theme) -> gpui::Div {
    let d = div()
        .h_full()
        .flex()
        .items_center()
        .gap(px(4.))
        .px(px(8.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis();
    if spec.grow {
        d.flex_1().min_w(px(width))
    } else {
        d.w(px(width)).flex_none()
    }
}

impl Sidebar {
    /// Column titles: a click sorts (the arrow shows the direction), the edge
    /// drags to resize.
    fn render_table_header(
        &self,
        cols: &'static [ColSpec],
        widths: &[f32],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = *theme;
        div()
            .h(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .bg(hsla(t.surface))
            .border_y_1()
            .border_color(hsla(t.border))
            .text_size(px(11.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(hsla(t.muted))
            .children(cols.iter().enumerate().map(|(c, spec)| {
                let sorted = self.table.sort == Some(c);
                let arrow = if !sorted {
                    ""
                } else if self.table.descending {
                    "\u{2193}"
                } else {
                    "\u{2191}"
                };
                let w = widths.get(c).copied().unwrap_or(spec.width);
                table_cell(spec, w, &t)
                    .id(("side-col", c))
                    .relative()
                    .cursor_pointer()
                    .when(sorted, |d| d.text_color(hsla(t.text)))
                    .hover(|d| d.bg(hsla(t.hover)))
                    .on_click(cx.listener(move |v, _, _, cx| v.click_header(c, cx)))
                    .child(spec.title)
                    .child(arrow)
                    .child(
                        div()
                            .id(("side-col-edge", c))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(px(5.))
                            .cursor(gpui::CursorStyle::ResizeLeftRight)
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener(move |v, ev: &gpui::MouseDownEvent, _, cx| {
                                    v.resizing = Some((c, f32::from(ev.position.x)));
                                    cx.stop_propagation();
                                }),
                            ),
                    )
            }))
    }
}

impl Sidebar {
    /// A compact segment table under the list, as in the design's main window.
    fn render_segments(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let t = *theme;
        let open = settings::get(cx).segments_open;
        let rows: Vec<_> = if open {
            self.segments.iter().take(8).collect()
        } else {
            Vec::new()
        };
        div()
            .flex_none()
            .border_t_1()
            .border_color(hsla(t.border))
            .child(
                div()
                    .id("segments-toggle")
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(hsla(t.hover)))
                    .on_click(cx.listener(|_, _, _, cx| {
                        settings::update(cx, |s| s.segments_open = !s.segments_open);
                        cx.notify();
                    }))
                    .child(ui::icon(
                        if open {
                            "chevron-down"
                        } else {
                            "chevron-right"
                        },
                        12.,
                        hsla(t.muted),
                    ))
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
