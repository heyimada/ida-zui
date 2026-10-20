// Code views as tabs: a listing, graph, hex view or pseudocode opened in a tab
// of its own ("Open in new tab"), next to the main view. A tab opened this way
// is pinned: it keeps its own place and only moves on its own jumps (see
// `session::follows`). The Linear / Graph / Hex / Pseudocode control
// retargets whichever tab is in front.
use super::Workspace;
use crate::database::{DocTab, Mode};
use crate::docs::{CodeView, DocKey, DocView};
use crate::session::{Driver, ViewKind};
use crate::settings;
use gpui::{App, Context, SharedString, Window};
use ida_proto::Ea;

impl ViewKindLabel for ViewKind {
    fn label(&self) -> &'static str {
        match self {
            ViewKind::Listing => "Listing",
            ViewKind::Graph => "Graph",
            ViewKind::Hex => "Hex",
            ViewKind::Pseudocode => "Pseudocode",
        }
    }
}

pub trait ViewKindLabel {
    fn label(&self) -> &'static str;
}

/// The kind as stored.
fn kind_key(kind: ViewKind) -> &'static str {
    match kind {
        ViewKind::Listing => "listing",
        ViewKind::Graph => "graph",
        ViewKind::Hex => "hex",
        ViewKind::Pseudocode => "pseudo",
    }
}

fn kind_from_key(key: &str) -> Option<ViewKind> {
    Some(match key {
        "listing" => ViewKind::Listing,
        "graph" => ViewKind::Graph,
        "hex" => ViewKind::Hex,
        "pseudo" => ViewKind::Pseudocode,
        _ => return None,
    })
}

impl Workspace {
    /// The key of the open database in `settings.json`: its input file.
    fn tabs_key(&self, cx: &App) -> Option<String> {
        let s = self.session.read(cx);
        s.request.as_ref().map(|r| r.path.display().to_string())
    }

    /// Writes the code tabs and the panes of the active database to settings.
    pub(super) fn save_tabs(&mut self, cx: &mut Context<Self>) {
        if self.restoring_tabs {
            return;
        }
        let Some(key) = self.tabs_key(cx) else { return };
        let mut saved = settings::SavedTabs::default();
        let mut index_of = std::collections::HashMap::new();
        for d in &self.docs {
            if let DocView::Code(cv) = &d.view {
                index_of.insert(d.id, saved.tabs.len());
                saved.tabs.push(settings::SavedTab {
                    kind: kind_key(cv.kind()).to_owned(),
                    ea: cv
                        .current_ea(cx)
                        .unwrap_or_else(|| self.session.read(cx).cursor),
                    pinned: cv.pinned(cx),
                    right: self.right.contains(&d.id),
                });
            }
        }
        // Torn-off views come back docked next time.
        for f in &self.floating {
            saved.tabs.push(settings::SavedTab {
                kind: kind_key(f.view.kind()).to_owned(),
                ea: f
                    .view
                    .current_ea(cx)
                    .unwrap_or_else(|| self.session.read(cx).cursor),
                pinned: f.view.pinned(cx),
                right: false,
            });
        }
        saved.left_front = self.doc.and_then(|id| index_of.get(&id).copied());
        saved.right_front = self.right_front.and_then(|id| index_of.get(&id).copied());
        saved.focus_right = self.focus_right;
        settings::update(cx, move |s| {
            if saved.tabs.is_empty() {
                s.saved_tabs.remove(&key);
            } else {
                s.saved_tabs.insert(key, saved);
            }
        });
    }

    /// Reopens the tabs saved for this database. True when there were any.
    pub(super) fn restore_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.tabs_key(cx) else {
            return false;
        };
        let Some(saved) = settings::get(cx).saved_tabs.get(&key).cloned() else {
            return false;
        };
        self.restoring_tabs = true;
        let mut ids = Vec::new();
        for t in &saved.tabs {
            let Some(kind) = kind_from_key(&t.kind) else {
                ids.push(None);
                continue;
            };
            self.add_code_tab(kind, t.ea, t.pinned, t.right, window, cx);
            ids.push(Some(self.next_doc - 1));
        }
        let id_at = |i: Option<usize>| i.and_then(|i| ids.get(i).copied().flatten());
        self.doc = id_at(saved.left_front);
        self.right_front = id_at(saved.right_front).or_else(|| self.right.last().copied());
        self.focus_right = saved.focus_right && self.right_front.is_some();
        self.restoring_tabs = false;
        self.sync_views(cx);
        self.focus_main(window, cx);
        self.tabs_changed(cx);
        true
    }

    /// The tab in front in the pane that has the keyboard (`None`: the main view).
    pub(super) fn focused_front(&self) -> Option<u64> {
        if self.focus_right {
            self.right_front
        } else {
            self.doc
        }
    }

    /// The tabs of a pane, left to right; the main view is `None` in the left pane.
    pub(super) fn strip(&self, right: bool) -> Vec<Option<u64>> {
        let mut out: Vec<Option<u64>> = Vec::new();
        if !right {
            out.push(None);
        }
        out.extend(
            self.docs
                .iter()
                .filter(|d| self.right.contains(&d.id) == right)
                .map(|d| Some(d.id)),
        );
        out
    }

    /// A tab opened, moved or closed: the layout flag and the saved tabs follow.
    pub(super) fn tabs_changed(&mut self, cx: &mut Context<Self>) {
        self.split = !self.right.is_empty();
        self.save_layout(cx);
        self.save_tabs(cx);
    }

    /// Opens a code view in the right pane (creating it).
    pub(super) fn open_in_right(
        &mut self,
        kind: ViewKind,
        ea: Ea,
        pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        self.add_code_tab(kind, ea, pinned, true, window, cx);
    }

    /// Both panes' tabs return to one pane.
    pub(super) fn close_right_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.right.clear();
        self.right_front = None;
        self.focus_right = false;
        self.sync_views(cx);
        self.focus_main(window, cx);
        self.tabs_changed(cx);
        cx.notify();
    }

    /// Tab (with two panes): the keyboard goes to the other pane.
    pub(super) fn focus_other_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to_right = !self.focus_right;
        if to_right && self.right_front.is_none() {
            return;
        }
        self.focus_right = to_right;
        self.sync_views(cx);
        self.focus_main(window, cx);
        cx.notify();
    }

    /// Moves a tab to a pane, before `before` (the end when `None`).
    pub(super) fn move_tab(
        &mut self,
        id: u64,
        to_right: bool,
        before: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.docs.iter().any(|d| d.id == id) {
            return;
        }
        let from_right = self.right.contains(&id);
        let was_front = if from_right {
            self.right_front == Some(id)
        } else {
            self.doc == Some(id)
        };
        let strip_before = self.strip(from_right);
        // The tab list is the order of both strips.
        let mut order: Vec<u64> = self.docs.iter().map(|d| d.id).collect();
        crate::panes::reorder(&mut order, id, before);
        self.docs
            .sort_by_key(|d| order.iter().position(|&t| t == d.id).unwrap_or(usize::MAX));
        if from_right != to_right {
            if was_front {
                // The pane it leaves shows its neighbour.
                let next = crate::panes::after_close(&strip_before, id);
                if from_right {
                    self.right_front = next.flatten();
                } else {
                    self.doc = next.flatten();
                }
            }
            if to_right {
                self.right.push(id);
            } else {
                self.right.retain(|&t| t != id);
            }
        }
        if self.right.is_empty() {
            self.right_front = None;
        }
        // The moved tab is in front where it lands.
        if to_right {
            self.right_front = Some(id);
            self.focus_right = true;
        } else {
            self.doc = Some(id);
            self.focus_right = false;
        }
        self.sync_views(cx);
        self.focus_main(window, cx);
        self.tabs_changed(cx);
        cx.notify();
    }

    /// View > Split right / "Move tab to the other pane".
    pub(super) fn move_to_other_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.focused_front() {
            Some(id) => {
                let to_right = !self.right.contains(&id);
                self.move_tab(id, to_right, None, window, cx);
            }
            None => {
                // The main view stays; its place opens in the right pane too.
                let kind = self.active_kind().unwrap_or(ViewKind::Listing);
                let ea = self.session.read(cx).cursor;
                self.open_in_right(kind, ea, false, window, cx);
            }
        }
    }

    /// Ctrl+PgDn / Ctrl+PgUp: the next or previous tab of the focused pane.
    pub(super) fn cycle_tab(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let strip = self.strip(self.focus_right);
        match crate::panes::cycle(&strip, self.focused_front(), dir) {
            Some(id) => self.show_doc(id, window, cx),
            None if !self.focus_right => self.show_main(window, cx),
            None => {}
        }
    }

    /// The code view in front when a code tab is, with its tab id.
    pub(super) fn front_code(&self) -> Option<(u64, CodeView)> {
        let id = self.focused_front()?;
        match &self.docs.iter().find(|d| d.id == id)?.view {
            DocView::Code(cv) => Some((id, cv.clone())),
            _ => None,
        }
    }

    /// The kind of the code view the user is working in: the tab in front, else
    /// the main view.
    pub(super) fn active_kind(&self) -> Option<ViewKind> {
        if let Some((_, cv)) = self.front_code() {
            return Some(cv.kind());
        }
        (!self.focus_right && self.doc.is_none()).then_some(match self.mode {
            Mode::Linear => ViewKind::Listing,
            Mode::Graph => ViewKind::Graph,
            Mode::Hex => ViewKind::Hex,
            Mode::Pseudo => ViewKind::Pseudocode,
        })
    }

    /// The tab's title: its kind and the function it is in.
    pub(super) fn code_tab_title(&self, view: &CodeView, cx: &App) -> SharedString {
        let s = self.session.read(cx);
        let at = match view.current_ea(cx) {
            Some(ea) => match s.function_at(ea) {
                Some(f) => crate::workspace::chrome::short_name(&f.name),
                None => format!("{ea:X}"),
            },
            None => String::new(),
        };
        format!("{}: {at}", view.kind().label()).into()
    }

    /// Opens a code view of `kind` at `ea` in a new tab (pinned: it stays
    /// where it is while the other views follow the user around).
    pub(super) fn open_code_tab(
        &mut self,
        kind: ViewKind,
        ea: Ea,
        pinned: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        // Into the pane that has the keyboard.
        let right = self.focus_right;
        self.add_code_tab(kind, ea, pinned, right, window, cx);
    }

    pub(super) fn add_code_tab(
        &mut self,
        kind: ViewKind,
        ea: Ea,
        pinned: bool,
        right: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.clone();
        let view = CodeView::open(kind, &session, ea, pinned, cx);
        let id = self.next_doc;
        self.next_doc += 1;
        // The tab needs a subscription like every tab; the code views report
        // through the session, so this one has nothing to forward.
        let sub = cx.observe(&session, |_, _, _| {});
        self.docs.push(DocTab {
            id,
            key: DocKey::Code(id),
            title: SharedString::default(),
            view: DocView::Code(view),
            _sub: sub,
        });
        if right {
            self.right.push(id);
        }
        self.show_doc(id, window, cx);
        self.tabs_changed(cx);
    }

    /// "New ... tab": a pinned view of `kind` at the cursor.
    pub(super) fn new_tab_here(
        &mut self,
        kind: ViewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ea = self.session.read(cx).cursor;
        self.open_code_tab(kind, ea, true, window, cx);
    }

    /// The tab in front becomes a view of `kind` at the same place (the
    /// Linear / Graph / Hex / Pseudocode control).
    pub(super) fn retarget_code_tab(
        &mut self,
        kind: ViewKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((id, old)) = self.front_code() else {
            return false;
        };
        if old.kind() == kind {
            return true;
        }
        let ea = old
            .current_ea(cx)
            .unwrap_or_else(|| self.session.read(cx).cursor);
        let pinned = old.pinned(cx);
        old.set_visible(false, cx);
        let session = self.session.clone();
        let view = CodeView::open(kind, &session, ea, pinned, cx);
        if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
            d.view = DocView::Code(view);
        }
        self.show_doc(id, window, cx);
        true
    }

    /// Tells the views and the inspector which view is in front: the one the
    /// user drives (see `session::follows`), and the graph the inspector reads.
    pub(super) fn sync_drive(db: &crate::database::Database, cx: &mut Context<Self>) {
        let front_id = if db.focus_right {
            db.right_front
        } else {
            db.doc
        };
        let front = front_id.and_then(|id| db.docs.iter().find(|d| d.id == id));
        let (driver, graph) = match front {
            Some(DocTab {
                view: DocView::Code(cv),
                ..
            }) => (
                Some(Driver {
                    origin: cv.origin(cx),
                    pinned: cv.pinned(cx),
                }),
                match cv {
                    CodeView::Graph(g) => Some(g.clone()),
                    _ => None,
                },
            ),
            Some(_) => (None, None),
            None if db.focus_right => (None, None),
            None => {
                let origin = match db.mode {
                    Mode::Linear => db.listing.read(cx).origin(),
                    Mode::Graph => db.graph.read(cx).origin(),
                    Mode::Hex => db.hex.read(cx).origin(),
                    Mode::Pseudo => db.pseudo.read(cx).origin(),
                };
                (
                    Some(Driver {
                        origin,
                        pinned: false,
                    }),
                    (db.mode == Mode::Graph).then(|| db.graph.clone()),
                )
            }
        };
        // The session cursor (status bar, breadcrumb, inspector) follows the view in front.
        let front_ea = match front {
            Some(DocTab {
                view: DocView::Code(cv),
                ..
            }) => cv
                .current_ea(cx)
                .map(|ea| (ea, cv.kind() == ViewKind::Pseudocode)),
            Some(_) => None,
            None if db.focus_right => None,
            None => match db.mode {
                Mode::Linear => db.listing.read(cx).current_ea().map(|e| (e, false)),
                Mode::Graph => db.graph.read(cx).current_ea().map(|e| (e, false)),
                Mode::Hex => db.hex.read(cx).current_ea().map(|e| (e, false)),
                Mode::Pseudo => db.pseudo.read(cx).current_ea().map(|e| (e, true)),
            },
        };
        db.session.update(cx, |s, cx| {
            s.driver = driver;
            if let (Some((ea, by_function)), Some(d)) = (front_ea, driver) {
                // A pseudocode view knows its function, not the exact address.
                let same_place = if by_function {
                    s.function_at(s.cursor).map(|f| f.start) == Some(ea)
                } else {
                    s.cursor == ea
                };
                if !same_place {
                    s.set_cursor(ea, d.origin, cx);
                }
            }
        });
        let shows_graph = graph.is_some();
        db.inspector.update(cx, |i, cx| {
            i.set_graph(graph.unwrap_or_else(|| db.graph.clone()), cx);
            i.block_mode = shows_graph;
            cx.notify();
        });
    }

    /// The code tabs show only while in front.
    pub(super) fn sync_code_tabs(db: &crate::database::Database, cx: &mut App) {
        for d in &db.docs {
            if let DocView::Code(cv) = &d.view {
                cv.set_visible(db.doc == Some(d.id) || db.right_front == Some(d.id), cx);
            }
        }
    }
}

/// A tab being dragged (reorder, or to the other pane).
#[derive(Clone)]
pub struct TabDrag {
    pub id: u64,
    pub title: SharedString,
}

impl gpui::Render for TabDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        use gpui::{ParentElement, Styled, div, px};
        let t = crate::theme::Theme::get(cx);
        div()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(crate::theme::hsla(t.popover))
            .border_1()
            .border_color(crate::theme::hsla(t.border_strong))
            .text_size(px(12.))
            .text_color(crate::theme::hsla(t.text))
            .child(self.title.clone())
    }
}
