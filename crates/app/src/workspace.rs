// The window's root: state, global commands and modal management. Layout
// pieces (title strip, toolbar, rail, tab strip, status bar, welcome) are in
// `chrome`; the panels are entities of their own.
mod chrome;
mod debug;

use crate::actions::*;
use crate::bottom::{BottomEvent, BottomPanel, BottomTab};
use crate::database::{Database, DocTab, Mode, TabKind};
use crate::dialog::{ConfirmDialog, DialogEvent};
use crate::docs::{DocEvent, DocKey, DocView};
use crate::enum_view::EnumView;
use crate::focus::Focus;
use crate::frame_view::FrameView;
use crate::graph::GraphView;
use crate::help::{AboutDialog, ShortcutsDialog};
use crate::hex::HexView;
use crate::inspector::{Inspector, InspectorEvent, InspectorTab};
use crate::listing::ListingView;
use crate::menubar::{BAR_HEIGHT, DbEntry, MenuBar, MenuEvent, Target, ViewState};
use crate::modal::{Choice, ChoiceValue, Modal, ModalEvent, Style as ModalStyle};
use crate::open_dialog::{OpenDialog, OpenRequest};
use crate::plugins::{self, RunPlugin};
use crate::plugins_view::{PluginsView, RunRequested};
use crate::proximity_view::ProximityView;
use crate::pseudo::PseudoView;
use crate::script_dialog::ScriptDialog;
use crate::session::{Phase, Session, SessionEvent, SubmitFn};
use crate::settings;
use crate::settings_view::{Section, SettingsView};
use crate::sidebar::{Sidebar, Tab};
use crate::sigs_view::SigsView;
use crate::struct_view::StructView;
use crate::theme::{Theme, UI_FONT, hsla, hsla_a};
use crate::types_view::TypesView;
use chrome::{RAIL_W, STATUS_H, TITLE_H};
use gpui::{
    AnyView, App, AppContext, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    PathPromptOptions, SharedString, Subscription, Window, div, prelude::*, px,
};
use ida_proto::{
    Ea, FindBytes, FindText, Lumina, LuminaAction, LuminaStatus, PluginList, PluginRun, ResumeHow,
    XrefsFrom, XrefsTo,
};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

enum ModalKind {
    Prompt(SubmitFn),
    Goto,
    Pick,
    /// A list of processes; the pick is a pid.
    Attach,
}

/// The panel edge being dragged.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    Sidebar,
    Inspector,
    Bottom,
}

struct OpenModal {
    view: Entity<Modal>,
    kind: ModalKind,
    return_focus: Option<FocusHandle>,
    _sub: Subscription,
}

/// A dialog card open over the workspace (settings, open options, ...).
struct ActiveDialog {
    view: AnyView,
    return_focus: Option<FocusHandle>,
    _sub: Subscription,
}

type Answer = Box<dyn FnOnce(&mut Workspace, usize, &mut Window, &mut Context<Workspace>)>;

/// The window: a strip of open databases (each with its own session and
/// views, see `database`) and everything shared between them. The active
/// database's fields read straight through `Deref`, so the code for one tab
/// does not care that there are others.
pub struct Workspace {
    dbs: Vec<Database>,
    active: usize,
    next_db: u64,
    menubar: Entity<MenuBar>,
    dialog: Option<ActiveDialog>,
    /// Called with the button picked in the open question dialog.
    pending_answer: Option<Answer>,
    /// Listing (or graph, or hex) and pseudocode side by side.
    split: bool,
    show_sidebar: bool,
    show_inspector: bool,
    show_bottom: bool,
    sidebar_w: f32,
    inspector_w: f32,
    bottom_h: f32,
    resizing: Option<Resize>,
    modal: Option<OpenModal>,
    close_confirmed: bool,
    /// A save prompt or a close is in flight; further close requests wait.
    busy: bool,
    /// The listing columns last pushed to the open databases.
    applied_listing: (u32, u32),
    /// Run requests from open Plugins windows.
    plugin_run_subs: Vec<Subscription>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Deref for Workspace {
    type Target = Database;

    fn deref(&self) -> &Database {
        &self.dbs[self.active]
    }
}

impl DerefMut for Workspace {
    fn deref_mut(&mut self) -> &mut Database {
        &mut self.dbs[self.active]
    }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let layout = settings::get(cx).layout.clone();
        let menubar = cx.new(|cx| MenuBar::new(window, cx));
        let menu_sub =
            cx.subscribe_in(
                &menubar,
                window,
                |this, _, ev: &MenuEvent, window, cx| match ev {
                    MenuEvent::Invoke(target) => this.run_menu_target(target.clone(), window, cx),
                },
            );
        let db = Self::make_db(1, window, cx);
        let applied_listing = {
            let st = settings::get(cx);
            (st.listing_indent, st.listing_comment)
        };
        Self {
            dbs: vec![db],
            active: 0,
            next_db: 2,
            menubar,
            dialog: None,
            pending_answer: None,
            split: layout.split,
            show_sidebar: layout.sidebar,
            show_inspector: layout.inspector,
            show_bottom: layout.bottom,
            sidebar_w: layout.sidebar_w,
            inspector_w: layout.inspector_w,
            bottom_h: layout.bottom_h,
            resizing: None,
            modal: None,
            close_confirmed: false,
            busy: false,
            applied_listing,
            plugin_run_subs: Vec::new(),
            focus: cx.focus_handle(),
            _subs: vec![menu_sub],
        }
    }

    /// A database with a session of its own and a view of everything in it.
    fn make_db(id: u64, window: &mut Window, cx: &mut Context<Self>) -> Database {
        let layout = settings::get(cx).layout.clone();
        let session = cx.new(|_| Session::new());
        let listing = cx.new(|cx| ListingView::new(session.clone(), cx));
        let pseudo = cx.new(|cx| PseudoView::new(session.clone(), cx));
        let graph = cx.new(|cx| GraphView::new(session.clone(), cx));
        let hex = cx.new(|cx| HexView::new(session.clone(), cx));
        let sidebar = cx.new(|cx| Sidebar::new(session.clone(), cx));
        sidebar.update(cx, |s, cx| s.select_tab(Tab::from_key(&layout.rail), cx));
        let focus_ctx = cx.new(|cx| Focus::new(session.clone(), cx));
        let inspector = cx.new(|cx| {
            Inspector::new(
                session.clone(),
                focus_ctx.clone(),
                graph.clone(),
                InspectorTab::from_key(&layout.inspector_tab),
                cx,
            )
        });
        let bottom = cx.new(|cx| {
            BottomPanel::new(
                session.clone(),
                focus_ctx.clone(),
                BottomTab::from_key(&layout.bottom_tab),
                cx,
            )
        });
        let inspector_sub = cx.subscribe(&inspector, |this, _, _: &InspectorEvent, cx| {
            this.save_layout(cx)
        });
        let bottom_sub = cx.subscribe_in(
            &bottom,
            window,
            |this, _, ev: &BottomEvent, window, cx| match ev {
                BottomEvent::TabChanged => this.save_layout(cx),
                BottomEvent::Close => {
                    this.show_bottom = false;
                    this.save_layout(cx);
                    cx.notify();
                }
                BottomEvent::RunScript => this.script_dialog(window, cx),
            },
        );
        let sub = cx.subscribe_in(&session, window, move |this, _, ev, window, cx| {
            this.on_session_event(id, ev, window, cx)
        });
        // A background tab repaints the window only when its chip changes.
        let observe = cx.observe(&session, move |this, _, cx| this.session_changed(id, cx));
        Database {
            id,
            session,
            listing,
            graph,
            hex,
            pseudo,
            mode: Mode::Linear,
            doc: None,
            docs: Vec::new(),
            next_doc: 1,
            sidebar,
            focus_ctx,
            inspector,
            bottom,
            goto_choices: None,
            pending_goto: None,
            chip: (TabKind::Empty, false, 0),
            _subs: vec![sub, observe, inspector_sub, bottom_sub],
        }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.dbs.iter().position(|d| d.id == id)
    }

    fn session_changed(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else { return };
        let sig = self.dbs[ix].signature(cx);
        if ix == self.active || self.dbs[ix].chip != sig {
            self.dbs[ix].chip = sig;
            cx.notify();
        }
    }

    fn on_session_event(
        &mut self,
        id: u64,
        ev: &SessionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.index_of(id) else { return };
        let active = ix == self.active;
        match ev {
            SessionEvent::Prompt(req) if active => {
                let req = req.clone();
                self.open_modal(
                    req.title.clone(),
                    &req.initial,
                    &req.placeholder,
                    Rc::default(),
                    true,
                    ModalKind::Prompt(req.on_submit.clone()),
                    ModalStyle::Prompt {
                        confirm: req.confirm.clone(),
                        validate_name: req.name,
                    },
                    window,
                    cx,
                );
            }
            SessionEvent::ShowXrefs { ea, to } if active => self.show_xrefs(*ea, *to, window, cx),
            SessionEvent::ShowXrefList { title, rows } if active => {
                self.show_xref_list(title.clone(), rows.clone(), window, cx)
            }
            SessionEvent::OpenDoc(key) if active => self.open_doc(key.clone(), window, cx),
            SessionEvent::DebugStopped => self.on_debug_stopped(ix, cx),
            SessionEvent::Closed => {
                self.clear_docs(ix, window, cx);
                if active {
                    self.set_window_title(window, cx);
                }
            }
            SessionEvent::ListsReloaded => self.dbs[ix].goto_choices = None,
            SessionEvent::Opened => {
                self.load_plugins(ix, cx);
                self.clear_docs(ix, window, cx);
                self.record_recent(ix, cx);
                Self::sync_db_views(&self.dbs[ix], self.split, cx);
                if active {
                    self.set_window_title(window, cx);
                    self.focus_main(window, cx);
                }
                if let Some(expr) = self.dbs[ix].pending_goto.take() {
                    self.dbs[ix]
                        .session
                        .update(cx, |s, cx| s.goto_expr(expr, cx));
                }
            }
            _ => {}
        }
    }

    /// "file — ida-zui" for the OS (taskbar, alt-tab).
    fn set_window_title(&self, window: &mut Window, cx: &App) {
        let db = &self.dbs[self.active];
        let title = if db.kind(cx) == TabKind::Ready {
            format!("{} — ida-zui", db.title(cx))
        } else {
            "ida-zui".to_owned()
        };
        window.set_window_title(&title);
    }

    // ---- layout state -------------------------------------------------

    /// Writes the panel layout to settings.json.
    fn save_layout(&mut self, cx: &mut Context<Self>) {
        let rail = self.sidebar.read(cx).tab().key().to_owned();
        let inspector_tab = self.inspector.read(cx).tab.key().to_owned();
        let bottom_tab = self.bottom.read(cx).tab.key().to_owned();
        let (split, sidebar, inspector, bottom) = (
            self.split,
            self.show_sidebar,
            self.show_inspector,
            self.show_bottom,
        );
        let (sidebar_w, inspector_w, bottom_h) = (self.sidebar_w, self.inspector_w, self.bottom_h);
        settings::update(cx, move |s| {
            s.layout = settings::PanelLayout {
                sidebar,
                inspector,
                bottom,
                split,
                sidebar_w,
                inspector_w,
                bottom_h,
                rail,
                bottom_tab,
                inspector_tab,
            };
        });
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.show_sidebar = !self.show_sidebar;
        self.save_layout(cx);
        cx.notify();
    }

    fn toggle_inspector(&mut self, cx: &mut Context<Self>) {
        self.show_inspector = !self.show_inspector;
        self.save_layout(cx);
        cx.notify();
    }

    fn toggle_bottom(&mut self, cx: &mut Context<Self>) {
        self.show_bottom = !self.show_bottom;
        self.save_layout(cx);
        cx.notify();
    }

    /// Opens the bottom panel on `tab`; a second request for the tab in front closes it.
    fn toggle_bottom_tab(&mut self, tab: BottomTab, cx: &mut Context<Self>) {
        let showing = self.show_bottom && self.bottom.read(cx).tab == tab;
        self.show_bottom = !showing;
        if !showing {
            self.bottom.update(cx, |b, cx| {
                b.tab = tab;
                cx.notify();
            });
        }
        self.save_layout(cx);
        cx.notify();
    }

    fn view_state(&self, cx: &App) -> ViewState {
        let dbg = &self.session.read(cx).dbg;
        ViewState {
            sidebar: self.show_sidebar,
            inspector: self.show_inspector,
            bottom: self.show_bottom,
            split: self.split,
            debug_active: dbg.active(),
            debug_suspended: dbg.suspended(),
        }
    }

    /// Tells the views which of them are on screen (the decompiler and the
    /// graph only work while shown).
    fn sync_views(&mut self, cx: &mut Context<Self>) {
        Self::sync_db_views(&self.dbs[self.active], self.split, cx);
    }

    fn sync_db_views(db: &Database, split: bool, cx: &mut Context<Self>) {
        let main = db.doc.is_none();
        let pseudo = main && (split || db.mode == Mode::Pseudo);
        let graph = main && db.mode == Mode::Graph;
        let hex = main && db.mode == Mode::Hex;
        db.pseudo.update(cx, |p, cx| p.set_visible(pseudo, cx));
        db.graph.update(cx, |g, cx| g.set_visible(graph, cx));
        db.hex.update(cx, |h, _| h.visible = hex);
        db.inspector.update(cx, |i, cx| {
            i.block_mode = graph;
            cx.notify();
        });
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = mode;
        self.doc = None;
        self.sync_views(cx);
        self.focus_main(window, cx);
        cx.notify();
    }

    /// Brings the main view forward (the first tab).
    fn show_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.doc = None;
        self.sync_views(cx);
        self.focus_main(window, cx);
        cx.notify();
    }

    fn show_doc(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.doc = Some(id);
        self.sync_views(cx);
        if let Some(d) = self.docs.iter().find(|d| d.id == id) {
            window.focus(&d.view.focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.split = !self.split;
        self.doc = None;
        self.sync_views(cx);
        self.focus_main(window, cx);
        self.save_layout(cx);
        cx.notify();
    }

    /// The view that has the keyboard for the current mode: the primary pane.
    fn main_focus(&self, cx: &App) -> FocusHandle {
        if let Some(id) = self.doc
            && let Some(d) = self.docs.iter().find(|d| d.id == id)
        {
            return d.view.focus_handle(cx);
        }
        match self.mode {
            Mode::Linear => self.listing.read(cx).focus_handle(cx),
            Mode::Graph => self.graph.read(cx).focus_handle(cx),
            Mode::Hex => self.hex.read(cx).focus_handle(cx),
            Mode::Pseudo => self.pseudo.read(cx).focus_handle(cx),
        }
    }

    fn focus_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let f = self.main_focus(cx);
        window.focus(&f, cx);
    }

    /// Tab / F5: open the pseudocode pane, or switch focus between the panes.
    fn toggle_pseudo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.is_some() {
            return;
        }
        let pseudo_focused = self
            .pseudo
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx);
        if !self.split {
            self.split = true;
            self.sync_views(cx);
            self.pseudo
                .update(cx, |p, cx| window.focus(&p.focus_handle(cx), cx));
            self.save_layout(cx);
        } else if pseudo_focused {
            // The other pane of the split: the listing when pseudocode is the main view.
            let f = if self.mode == Mode::Pseudo {
                self.listing.read(cx).focus_handle(cx)
            } else {
                self.main_focus(cx)
            };
            window.focus(&f, cx);
        } else {
            self.pseudo
                .update(cx, |p, cx| window.focus(&p.focus_handle(cx), cx));
        }
        cx.notify();
    }

    // ---- databases -----------------------------------------------------------

    /// Opens `path` in a tab of its own (or brings its tab forward).
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let mut req = OpenRequest::auto(path);
        req.run_auto = settings::get(cx).analysis_on_open;
        self.open_request(req, window, cx);
    }

    /// `ida-zui a.exe b.dll --goto main`: every file gets a tab, `goto` applies
    /// to the first, and the first tab ends up in front.
    pub fn open_files(
        &mut self,
        files: Vec<PathBuf>,
        goto: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut goto = goto;
        let mut first = None;
        for path in files {
            self.open_path(path, window, cx);
            first.get_or_insert(self.active);
            if let Some(expr) = goto.take() {
                self.pending_goto = Some(expr);
            }
        }
        if let Some(ix) = first {
            self.activate(ix, window, cx);
        }
    }

    /// Opens with loader options. The active tab is reused when it is empty
    /// (or its last open failed); otherwise the database gets a new tab. A
    /// database that is already open just comes forward, since two hosts on
    /// one .i64 would fight over it.
    fn open_request(&mut self, req: OpenRequest, window: &mut Window, cx: &mut Context<Self>) {
        let same = |a: &std::path::Path, b: &std::path::Path| {
            a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
        };
        let want = req.database_path();
        let open = self.dbs.iter().position(|d| {
            let s = d.session.read(cx);
            matches!(s.phase, Phase::Loading { .. } | Phase::Ready)
                && s.request
                    .as_ref()
                    .is_some_and(|r| same(&r.database_path(), &want))
        });
        if let Some(ix) = open {
            self.activate(ix, window, cx);
            self.session
                .update(cx, |s, cx| s.info_msg("That database is already open", cx));
            return;
        }
        if !matches!(
            self.dbs[self.active].kind(cx),
            TabKind::Empty | TabKind::Failed
        ) {
            let id = self.next_db;
            self.next_db += 1;
            let db = Self::make_db(id, window, cx);
            self.dbs.push(db);
            self.active = self.dbs.len() - 1;
        }
        self.session.update(cx, |s, cx| s.open_with(req, cx));
        self.set_window_title(window, cx);
        self.sync_views(cx);
        self.focus_main(window, cx);
        // Tick while loading so the elapsed time stays live.
        let id = self.id;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let loading = this
                    .update(cx, |w, cx| {
                        let Some(ix) = w.index_of(id) else {
                            return false;
                        };
                        let loading =
                            matches!(w.dbs[ix].session.read(cx).phase, Phase::Loading { .. });
                        if loading {
                            cx.notify();
                        }
                        loading
                    })
                    .unwrap_or(false);
                if !loading {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// Indices of the databases that have a tab in the strip.
    fn tabs(&self, cx: &App) -> Vec<usize> {
        (0..self.dbs.len())
            .filter(|&i| self.dbs[i].kind(cx) != TabKind::Empty)
            .collect()
    }

    /// Brings database `ix` forward. Ignored while a dialog or overlay is up,
    /// since those act on the database that was in front when they opened.
    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.dbs.len()
            || ix == self.active
            || self.modal.is_some()
            || self.dialog.is_some()
        {
            return;
        }
        self.menubar.update(cx, |m, cx| m.close(window, cx));
        self.active = ix;
        self.set_window_title(window, cx);
        // Each tab keeps its own view flags; this only catches up with a
        // split toggled while the tab was in the background.
        self.sync_views(cx);
        self.focus_main(window, cx);
        cx.notify();
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab.
    fn cycle_db(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let tabs = self.tabs(cx);
        let Some(at) = tabs.iter().position(|&i| i == self.active) else {
            return;
        };
        let next = (at as isize + delta).rem_euclid(tabs.len() as isize) as usize;
        self.activate(tabs[next], window, cx);
    }

    /// Ctrl+1..9: the nth tab.
    fn activate_nth(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&ix) = self.tabs(cx).get(n) {
            self.activate(ix, window, cx);
        }
    }

    /// Pushes the listing columns from the settings to every open database.
    fn apply_listing_options(&mut self, cx: &mut Context<Self>) {
        let (indent, comment) = {
            let st = settings::get(cx);
            (st.listing_indent, st.listing_comment)
        };
        if (indent, comment) == self.applied_listing {
            return;
        }
        self.applied_listing = (indent, comment);
        for d in &self.dbs {
            d.session
                .update(cx, |s, cx| s.set_listing_options(indent, comment, cx));
        }
    }

    // ---- editor tabs ---------------------------------------------------

    /// Opens the editor `key` names in a tab, or brings the tab that holds it
    /// forward.
    fn open_doc(&mut self, key: DocKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        if let Some(d) = self.docs.iter().find(|d| d.key.same_tab(&key)) {
            let id = d.id;
            if let (DocKey::LocalTypes(Some(name)), DocView::Types(v)) = (&key, &d.view) {
                v.update(cx, |v, cx| v.select_type(name.clone(), cx));
            }
            self.show_doc(id, window, cx);
            return;
        }
        let id = self.next_doc;
        self.next_doc += 1;
        let session = self.session.clone();
        let (view, title, sub) = match &key {
            DocKey::LocalTypes(select) => {
                let select = select.clone();
                let v = cx.new(|cx| TypesView::new(session, select, window, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                (DocView::Types(v), SharedString::from("Local types"), sub)
            }
            DocKey::Struct(name) => {
                let name = name.clone();
                let v = cx.new(|cx| StructView::new(session, name.clone(), cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                (DocView::Struct(v), SharedString::from(name), sub)
            }
            DocKey::Enum(name) => {
                let name = name.clone();
                let v = cx.new(|cx| EnumView::new(session, name.clone(), cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                (DocView::Enum(v), SharedString::from(name), sub)
            }
            DocKey::Frame(func) => {
                let func = *func;
                let name = self
                    .session
                    .read(cx)
                    .function_at(func)
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                let v = cx.new(|cx| FrameView::new(session, func, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                (
                    DocView::Frame(v),
                    SharedString::from(format!("Frame: {name}")),
                    sub,
                )
            }
            DocKey::Signatures => {
                let v = cx.new(|cx| SigsView::new(session, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                (DocView::Sigs(v), SharedString::from("Signatures"), sub)
            }
            DocKey::Plugins => {
                let v = cx.new(|cx| PluginsView::new(session, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                let run = cx.subscribe_in(&v, window, |this, _, ev: &RunRequested, window, cx| {
                    this.run_plugin(&ev.0, window, cx)
                });
                self.plugin_run_subs.push(run);
                (DocView::Plugins(v), SharedString::from("Plugins"), sub)
            }
            DocKey::Proximity(func) => {
                let func = *func;
                let name = self
                    .session
                    .read(cx)
                    .function_at(func)
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                let v = cx.new(|cx| ProximityView::new(session, func, cx));
                let sub = cx.subscribe_in(&v, window, move |this, _, ev: &DocEvent, window, cx| {
                    this.on_doc_event(id, ev, window, cx)
                });
                (
                    DocView::Proximity(v),
                    SharedString::from(format!("Proximity: {name}")),
                    sub,
                )
            }
        };
        self.docs.push(DocTab {
            id,
            key,
            title,
            view,
            _sub: sub,
        });
        self.show_doc(id, window, cx);
    }

    /// A picker over the structures (or enums) of the database.
    fn pick_type(&mut self, enums: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        let fut = self.session.read(cx).call(ida_proto::TypeList {});
        let handle = window.window_handle();
        let db = self.id;
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |w, cx| match result {
                    // The reply belongs to a tab that is no longer in front.
                    _ if w.id != db => {}
                    Ok(list) => {
                        let choices: Vec<Choice> = list
                            .into_iter()
                            .filter(|t| {
                                if enums {
                                    t.kind == "enum"
                                } else {
                                    matches!(t.kind.as_str(), "struct" | "union")
                                }
                            })
                            .map(|t| Choice {
                                value: ChoiceValue::Doc(if enums {
                                    DocKey::Enum(t.name.clone())
                                } else {
                                    DocKey::Struct(t.name.clone())
                                }),
                                label: t.name.into(),
                                detail: format!("{} members", t.members).into(),
                                kind: t.kind.into(),
                                group: SharedString::default(),
                                keywords: SharedString::default(),
                            })
                            .collect();
                        let title = if enums {
                            "Open an enum"
                        } else {
                            "Open a structure or union"
                        };
                        w.open_modal(
                            title,
                            "",
                            "Filter",
                            Rc::new(choices),
                            false,
                            ModalKind::Pick,
                            ModalStyle::Palette {
                                enter: "open",
                                key_hint: None,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(e) => w.session.update(cx, |s, cx| s.error(e.to_string(), cx)),
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// A Lumina request. A push uploads the analysis to a third-party server,
    /// so it is asked about first, with Cancel as the default answer; the host
    /// refuses a push that does not carry the confirmation.
    fn lumina(&mut self, action: LuminaAction, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        let single = matches!(action, LuminaAction::PullFunc | LuminaAction::PushFunc);
        let func = {
            let s = self.session.read(cx);
            s.function_at(s.cursor).map(|f| (f.start, f.name.clone()))
        };
        if single && func.is_none() {
            self.session.update(cx, |s, cx| {
                s.info_msg("The cursor is not inside a function", cx)
            });
            return;
        }
        let ea = func.as_ref().map_or(0, |f| f.0);
        let status = self.session.read(cx).call(LuminaStatus {});
        let db = self.id;
        cx.spawn_in(window, async move |this, cx| {
            let server = status.await.map(|s| s.server).unwrap_or_default();
            this.update_in(cx, |w, window, cx| {
                if w.id != db {
                    return;
                }
                if action.is_push() {
                    let text = crate::lumina::push_warning(
                        action,
                        &server,
                        func.as_ref().map(|f| f.1.as_str()),
                    );
                    let title = if single {
                        "Upload this function to Lumina?"
                    } else {
                        "Upload all metadata to Lumina?"
                    };
                    w.ask(
                        title,
                        text,
                        &["Cancel", "Upload to Lumina"],
                        0,
                        0,
                        window,
                        cx,
                        move |w, answer, window, cx| {
                            if answer == 1 {
                                w.lumina_run(action, ea, true, server, window, cx);
                            }
                        },
                    );
                } else {
                    w.lumina_run(action, ea, false, server, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Loads the database's plugin list into the shared registry: the Plugins
    /// menu, the palette and the keymap read it from there.
    fn load_plugins(&mut self, ix: usize, cx: &mut Context<Self>) {
        let fut = self.dbs[ix].session.read(cx).call(PluginList {});
        cx.spawn(async move |_, cx| {
            if let Ok(list) = fut.await {
                cx.update(|cx| plugins::set_list(list, cx));
            }
        })
        .detach();
    }

    /// Runs a plugin and reports what happened. A plugin that needs IDA's Qt
    /// UI does nothing here; the reply says so and a dialog explains it. A
    /// plugin still running after 20 s gets an interrupt, which stops Python
    /// plugins (native ones stop when they return).
    fn run_plugin(&mut self, action: &RunPlugin, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        let Some(info) = plugins::list()
            .into_iter()
            .find(|p| p.path == action.path && p.arg == action.arg)
        else {
            self.session
                .update(cx, |s, cx| s.error("That plugin is not in the list", cx));
            return;
        };
        let name = info.name.clone();
        self.session.update(cx, |s, cx| {
            s.info_msg(format!("Running plugin {name}…"), cx)
        });
        let fut = self.session.read(cx).call(PluginRun {
            plugin: action.path.clone(),
            arg: action.arg,
        });
        let done = Rc::new(std::cell::Cell::new(false));
        let (flag, session) = (done.clone(), self.session.clone());
        cx.spawn_in(window, async move |_, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(20))
                .await;
            if !flag.get()
                && let Ok(interrupt) =
                    cx.update(|_, cx| session.read(cx).call(ida_proto::Interrupt {}))
            {
                let _ = interrupt.await;
            }
        })
        .detach();
        let db = self.id;
        cx.spawn_in(window, async move |this, cx| {
            let result = fut.await;
            done.set(true);
            this.update_in(cx, |w, window, cx| {
                if w.id != db {
                    return;
                }
                let outcome = match result {
                    Ok(r) => {
                        if r.changed {
                            w.session.update(cx, |s, cx| s.changed(cx));
                        }
                        plugins::outcome(&name, &r)
                    }
                    Err(e) => plugins::Outcome {
                        ok: false,
                        short: format!("{name}: {e}"),
                        detail: None,
                    },
                };
                w.session.update(cx, |s, cx| {
                    if outcome.ok {
                        s.info_msg(outcome.short.clone(), cx)
                    } else {
                        s.error(outcome.short.clone(), cx)
                    }
                });
                for d in &w.docs {
                    if let DocView::Plugins(v) = &d.view {
                        v.update(cx, |v, cx| {
                            v.set_message(outcome.short.clone(), !outcome.ok, cx)
                        });
                    }
                }
                if let Some(detail) = outcome.detail {
                    w.ask(name, detail, &["Close"], 0, 0, window, cx, |_, _, _, _| {});
                }
            })
            .ok();
        })
        .detach();
    }

    fn lumina_run(
        &mut self,
        action: LuminaAction,
        ea: Ea,
        confirmed: bool,
        server: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = if server.is_empty() {
            "the Lumina server".to_owned()
        } else {
            server.clone()
        };
        self.session.update(cx, |s, cx| {
            s.info_msg(
                format!(
                    "Lumina: {} ({target})…",
                    crate::lumina::action_label(action)
                ),
                cx,
            )
        });
        let fut = self.session.read(cx).call(Lumina {
            action,
            ea,
            confirmed,
        });
        let db = self.id;
        cx.spawn_in(window, async move |this, cx| {
            let result = fut.await;
            this.update_in(cx, |w, window, cx| {
                if w.id != db {
                    return;
                }
                let outcome = match result {
                    Ok(r) => {
                        if r.lumina_after > r.lumina_before {
                            w.session.update(cx, |s, cx| s.changed(cx));
                        }
                        crate::lumina::outcome(action, &r, &server)
                    }
                    Err(e) => crate::lumina::Outcome {
                        title: "Lumina request refused".into(),
                        detail: e.to_string(),
                        ok: false,
                    },
                };
                w.session.update(cx, |s, cx| {
                    if outcome.ok {
                        s.info_msg(outcome.title.clone(), cx)
                    } else {
                        s.error(
                            format!(
                                "{}: {}",
                                outcome.title,
                                outcome.detail.lines().nth(2).unwrap_or("")
                            ),
                            cx,
                        )
                    }
                });
                w.ask(
                    outcome.title,
                    outcome.detail,
                    &["Close"],
                    0,
                    0,
                    window,
                    cx,
                    |_, _, _, _| {},
                );
            })
            .ok();
        })
        .detach();
    }

    fn on_doc_event(
        &mut self,
        id: u64,
        ev: &DocEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            DocEvent::Title(t) => {
                if let Some(d) = self.docs.iter_mut().find(|d| d.id == id) {
                    d.title = t.clone();
                    cx.notify();
                }
            }
            DocEvent::Open(key) => self.open_doc(key.clone(), window, cx),
            DocEvent::Navigate(ea) => {
                let ea = *ea;
                self.show_main(window, cx);
                self.session.update(cx, |s, cx| s.navigate(ea, cx));
            }
        }
    }

    fn close_doc(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.docs.iter().position(|d| d.id == id) else {
            return;
        };
        self.docs.remove(at);
        if self.doc == Some(id) {
            // Fall back to the neighbouring editor tab, else the main view.
            match at
                .checked_sub(1)
                .and_then(|i| self.docs.get(i))
                .map(|d| d.id)
            {
                Some(prev) => self.show_doc(prev, window, cx),
                None => self.show_main(window, cx),
            }
        }
        cx.notify();
    }

    /// Drops the editor tabs of database `ix` (its database closed or reopened).
    fn clear_docs(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let db = &mut self.dbs[ix];
        if db.docs.is_empty() {
            return;
        }
        db.docs.clear();
        let was_front = db.doc.take().is_some();
        if was_front && ix == self.active {
            self.show_main(window, cx);
        }
        cx.notify();
    }

    // ---- questions and dialogs -------------------------------------------

    /// Closes database `id` after asking what to do with unsaved changes:
    /// Save, Don't save or Cancel.
    fn close_db_checked(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(ix) = self.index_of(id) else { return };
        if !self.dbs[ix].dirty(cx) {
            self.close_db(id, false, window, cx);
            return;
        }
        // Show which database the question is about.
        self.activate(ix, window, cx);
        self.busy = true;
        let file = self.dbs[ix].title(cx);
        self.ask(
            format!("Save changes to {file}?"),
            "Your renames, comments and types are only in memory until the database is saved.",
            &["Save", "Don't save", "Cancel"],
            0,
            2,
            window,
            cx,
            move |w, choice, window, cx| {
                w.busy = false;
                if choice < 2 {
                    w.close_db(id, choice == 0, window, cx);
                }
            },
        );
    }

    /// Closes database `id` (saving it if asked) and removes its tab. The
    /// host shuts down in the background; the last tab makes way for an
    /// empty one, which shows the welcome screen.
    fn close_db(&mut self, id: u64, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else { return };
        // Dropping the task would cancel the save.
        self.dbs[ix]
            .session
            .update(cx, |s, cx| s.close(save, cx))
            .detach();
        self.dbs.remove(ix);
        if self.dbs.is_empty() {
            let id = self.next_db;
            self.next_db += 1;
            let db = Self::make_db(id, window, cx);
            self.dbs.push(db);
            self.active = 0;
        } else if ix < self.active {
            self.active -= 1;
        } else if ix == self.active {
            self.active = ix.min(self.dbs.len() - 1);
        }
        self.set_window_title(window, cx);
        self.sync_views(cx);
        self.focus_main(window, cx);
        cx.notify();
    }

    /// Window close hook. Asks once for every database with unsaved changes,
    /// closes all hosts through the protocol (so each is packed into its
    /// .i64), then removes the window once they have acknowledged. Returns
    /// true only when there is nothing to wait for.
    pub fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed || self.dbs.iter().all(|d| d.session.read(cx).client_is_none()) {
            return true;
        }
        if self.busy {
            return false;
        }
        let dirty: Vec<String> = self
            .dbs
            .iter()
            .filter(|d| d.dirty(cx))
            .map(|d| d.title(cx))
            .collect();
        if dirty.is_empty() {
            self.close_all(false, window, cx);
            return false;
        }
        self.busy = true;
        let (title, detail) = match dirty.as_slice() {
            [one] => (
                format!("Save changes to {one}?"),
                "Your renames, comments and types are only in memory until the database is saved."
                    .to_owned(),
            ),
            many => (
                format!("Save changes to {} databases?", many.len()),
                many.join(", "),
            ),
        };
        self.ask(
            title,
            detail,
            &["Save", "Don't save", "Cancel"],
            0,
            2,
            window,
            cx,
            |w, choice, window, cx| {
                w.busy = false;
                if choice < 2 {
                    w.close_all(choice == 0, window, cx);
                }
            },
        );
        false
    }

    /// Closes every database (saving the changed ones if `save`), then the window.
    fn close_all(&mut self, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        let tasks: Vec<gpui::Task<()>> = self
            .dbs
            .iter()
            .map(|d| {
                let save = save && d.dirty(cx);
                d.session.update(cx, |s, cx| s.close(save, cx))
            })
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            futures::future::join_all(tasks).await;
            this.update_in(cx, |w, window, _| {
                w.close_confirmed = true;
                window.remove_window();
            })
            .ok();
        })
        .detach();
    }

    /// An in-app question; `then` gets the 0-based button that answered it.
    #[allow(clippy::too_many_arguments)]
    fn ask(
        &mut self,
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        buttons: &[&str],
        default: usize,
        cancel: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, usize, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let dialog = cx.new(|cx| ConfirmDialog::new(title, detail, buttons, default, cancel, cx));
        self.pending_answer = Some(Box::new(then));
        self.show_dialog(dialog, window, cx);
    }

    /// Puts `view` over the workspace and focuses it.
    fn show_dialog<D>(&mut self, view: Entity<D>, window: &mut Window, cx: &mut Context<Self>)
    where
        D: Render + Focusable + EventEmitter<DialogEvent>,
    {
        let return_focus = self
            .dialog
            .as_ref()
            .and_then(|d| d.return_focus.clone())
            .or_else(|| window.focused(cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &DialogEvent, window, cx| {
            this.on_dialog_event(ev, window, cx)
        });
        let focus = view.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.dialog = Some(ActiveDialog {
            view: view.into(),
            return_focus,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.dialog.take() {
            match d.return_focus {
                Some(f) => window.focus(&f, cx),
                None => self.focus_main(window, cx),
            }
        }
        cx.notify();
    }

    fn on_dialog_event(&mut self, ev: &DialogEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            DialogEvent::Close => {
                self.close_dialog(window, cx);
                self.pending_answer = None;
            }
            DialogEvent::Answer(i) => {
                let then = self.pending_answer.take();
                self.close_dialog(window, cx);
                if let Some(then) = then {
                    then(self, *i, window, cx);
                }
            }
            DialogEvent::Open(req) => {
                let req = req.clone();
                self.close_dialog(window, cx);
                self.open_request(req, window, cx);
            }
            DialogEvent::RunScript(path) => {
                let path = path.clone();
                self.close_dialog(window, cx);
                self.show_python(window, cx);
                let console = self.bottom.read(cx).console.clone();
                console.update(cx, |c, cx| c.run_file(path, cx));
            }
        }
    }

    fn script_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        let dialog = cx.new(ScriptDialog::new);
        self.show_dialog(dialog, window, cx);
    }

    /// Opens the bottom panel on the Python tab with the caret in the console.
    fn show_python(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_bottom = true;
        self.bottom.update(cx, |b, cx| {
            b.tab = BottomTab::Python;
            cx.notify();
        });
        self.save_layout(cx);
        cx.notify();
        let console = self.bottom.read(cx).console.clone();
        // The panel may only now be laid out; focus once it is.
        window.defer(cx, move |window, cx| {
            console.update(cx, |c, cx| c.focus(window, cx))
        });
    }

    fn open_options_dialog(
        &mut self,
        path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| OpenDialog::new(path.as_deref(), cx));
        self.show_dialog(dialog, window, cx);
    }

    fn open_settings(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| SettingsView::new(section, cx));
        self.show_dialog(dialog, window, cx);
    }

    /// Adds the database that just opened to the recent list: the file the
    /// user chose, or the database itself when options changed how it loads
    /// (opening that file plainly would load it differently).
    fn record_recent(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(req) = self.dbs[ix].session.read(cx).request.clone() else {
            return;
        };
        let path = if req.is_plain() {
            req.path
        } else {
            req.database_path()
        };
        settings::update(cx, |s| s.touch_recent(&path, settings::unix_now()));
    }

    fn run_menu_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            Target::Action(action) => window.dispatch_action(action, cx),
            Target::Theme(name) => settings::update(cx, |s| s.theme = name.to_owned()),
            Target::CodeFontSize(size) => settings::update(cx, |s| s.code_font_size = size),
            Target::Recent(path) => self.open_path(PathBuf::from(path), window, cx),
            Target::ClearRecent => settings::update(cx, |s| s.recent.clear()),
            Target::Database(id) => {
                if let Some(ix) = self.index_of(id) {
                    self.activate(ix, window, cx);
                }
            }
        }
    }

    fn show_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.show_sidebar = true;
        self.sidebar.update(cx, |s, cx| s.show_tab(tab, window, cx));
        cx.notify();
    }

    fn open_menu(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.menubar
            .update(cx, |m, cx| m.open_by_index(ix, window, cx));
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Open binary or database".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update_in(cx, |w, window, cx| {
                    for path in paths {
                        w.open_path(path, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    // ---- overlays ------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn open_modal(
        &mut self,
        title: impl Into<SharedString>,
        initial: &str,
        placeholder: &str,
        choices: Rc<Vec<Choice>>,
        prefer_text: bool,
        kind: ModalKind,
        style: ModalStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Modal> {
        let return_focus = window
            .focused(cx)
            .or_else(|| self.modal.as_ref().and_then(|m| m.return_focus.clone()));
        let view = cx.new(|cx| {
            Modal::new(
                title,
                initial,
                placeholder,
                choices,
                prefer_text,
                style,
                window,
                cx,
            )
        });
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &ModalEvent, window, cx| {
            this.on_modal_event(ev, window, cx)
        });
        self.modal = Some(OpenModal {
            view: view.clone(),
            kind,
            return_focus,
            _sub: sub,
        });
        cx.notify();
        view
    }

    fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.modal.take() {
            match m.return_focus {
                Some(f) => window.focus(&f, cx),
                None => self.focus_main(window, cx),
            }
        }
        cx.notify();
    }

    fn on_modal_event(&mut self, ev: &ModalEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.modal.as_ref() else {
            return;
        };
        match ev {
            ModalEvent::Dismiss => self.close_modal(window, cx),
            ModalEvent::Pick(ChoiceValue::Ea(pid)) if matches!(open.kind, ModalKind::Attach) => {
                let pid = *pid as u32;
                self.close_modal(window, cx);
                self.session.update(cx, |s, cx| s.dbg_attach(pid, cx));
            }
            ModalEvent::Pick(ChoiceValue::Ea(ea)) => {
                let ea = *ea;
                self.close_modal(window, cx);
                self.session.update(cx, |s, cx| s.navigate(ea, cx));
            }
            ModalEvent::Pick(ChoiceValue::Doc(key)) => {
                let key = key.clone();
                self.close_modal(window, cx);
                self.open_doc(key, window, cx);
            }
            ModalEvent::Pick(ChoiceValue::Action(action)) => {
                let action = action.boxed_clone();
                self.close_modal(window, cx);
                window.dispatch_action(action, cx);
            }
            ModalEvent::Submit(text) => {
                let text = text.clone();
                match &open.kind {
                    ModalKind::Prompt(f) => {
                        let f = f.clone();
                        self.close_modal(window, cx);
                        f(text, cx);
                    }
                    ModalKind::Goto => {
                        self.close_modal(window, cx);
                        self.session.update(cx, |s, cx| s.goto_expr(text, cx));
                    }
                    ModalKind::Pick | ModalKind::Attach => {}
                }
            }
        }
    }

    fn goto_choices(&mut self, cx: &App) -> Rc<Vec<Choice>> {
        if let Some(c) = &self.goto_choices {
            return c.clone();
        }
        let s = self.session.read(cx);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::with_capacity(s.functions.len() + s.names.len());
        for f in s.functions.iter() {
            seen.insert(f.start);
            out.push(Choice {
                label: f.name.clone().into(),
                detail: format!("{:X}", f.start).into(),
                kind: "fn".into(),
                group: "Functions".into(),
                keywords: SharedString::default(),
                value: ChoiceValue::Ea(f.start),
            });
        }
        for n in s.names.iter() {
            if seen.insert(n.ea) {
                out.push(Choice {
                    label: n.name.clone().into(),
                    detail: format!("{:X}", n.ea).into(),
                    kind: "name".into(),
                    group: "Names".into(),
                    keywords: SharedString::default(),
                    value: ChoiceValue::Ea(n.ea),
                });
            }
        }
        let rc = Rc::new(out);
        self.goto_choices = Some(rc.clone());
        rc
    }

    fn goto(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            return;
        }
        let choices = self.goto_choices(cx);
        let hint = shortcut::<GoTo>(cx)
            .split(" / ")
            .last()
            .map(|s| SharedString::from(s.to_owned()));
        self.open_modal(
            "Jump to address, name or expression",
            "",
            "Jump to address, name or expression",
            choices,
            false,
            ModalKind::Goto,
            ModalStyle::Palette {
                enter: "jump",
                key_hint: hint,
            },
            window,
            cx,
        );
    }

    fn palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.session.read(cx).is_ready();
        let choices = crate::actions::commands()
            .into_iter()
            .filter(|c| c.palette && (ready || !c.needs_db))
            .map(|c| {
                let action = (c.make)();
                Choice {
                    label: c.label.into(),
                    detail: crate::actions::shortcut_text(&*action, cx).into(),
                    kind: SharedString::default(),
                    group: c.group.into(),
                    keywords: c.keywords.into(),
                    value: ChoiceValue::Action(action),
                }
            })
            .collect();
        self.open_modal(
            "Command palette",
            "",
            "Type a command",
            Rc::new(choices),
            false,
            ModalKind::Pick,
            ModalStyle::Palette {
                enter: "run",
                key_hint: Some(shortcut::<CommandPalette>(cx).into()),
            },
            window,
            cx,
        );
    }

    fn xref_choice(x: ida_proto::XrefRow, to: bool) -> Choice {
        let at = if to { x.from } else { x.to };
        let kind = match x.kind.as_str() {
            "p" => "call",
            "j" => "jmp",
            "r" => "read",
            "w" => "write",
            "o" => "offs",
            "i" => "info",
            "^" => "flow",
            other => other,
        };
        Choice {
            label: x.func.unwrap_or_else(|| format!("{at:X}")).into(),
            detail: x.text.into(),
            kind: kind.to_owned().into(),
            group: SharedString::default(),
            keywords: SharedString::default(),
            value: ChoiceValue::Ea(at),
        }
    }

    fn show_xrefs(&mut self, ea: Ea, to: bool, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.session.read(cx);
        let fut = if to {
            futures::future::Either::Left(s.call(XrefsTo { ea }))
        } else {
            futures::future::Either::Right(s.call(XrefsFrom { ea }))
        };
        let window_handle = window.window_handle();
        let db = self.id;
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            cx.update_window(window_handle, |_, window, cx| {
                this.update(cx, |w, cx| match result {
                    _ if w.id != db => {}
                    Ok(rows) if rows.is_empty() => w.session.update(cx, |s, cx| {
                        s.info_msg(
                            format!(
                                "No cross references {} {ea:X}",
                                if to { "to" } else { "from" }
                            ),
                            cx,
                        )
                    }),
                    Ok(rows) => {
                        let choices = rows.into_iter().map(|x| Self::xref_choice(x, to)).collect();
                        let title =
                            format!("Cross references {} {ea:X}", if to { "to" } else { "from" });
                        w.open_modal(
                            title,
                            "",
                            "Filter",
                            Rc::new(choices),
                            false,
                            ModalKind::Pick,
                            ModalStyle::Palette {
                                enter: "open",
                                key_hint: None,
                            },
                            window,
                            cx,
                        );
                    }
                    Err(e) => w.session.update(cx, |s, cx| s.error(e.to_string(), cx)),
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a ready-made reference list in the pick overlay.
    fn show_xref_list(
        &mut self,
        title: String,
        rows: Vec<ida_proto::XrefRow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if rows.is_empty() {
            self.session.update(cx, |s, cx| {
                s.info_msg(format!("{title}: no references"), cx)
            });
            return;
        }
        let choices = rows
            .into_iter()
            .map(|x| Self::xref_choice(x, true))
            .collect();
        self.open_modal(
            title,
            "",
            "Filter",
            Rc::new(choices),
            false,
            ModalKind::Pick,
            ModalStyle::Palette {
                enter: "open",
                key_hint: None,
            },
            window,
            cx,
        );
    }

    fn search(&mut self, bytes: bool, cx: &mut Context<Self>) {
        let session = self.session.clone();
        let (title, hint) = if bytes {
            ("Search bytes (e.g. 48 8B ?? 24)", "")
        } else {
            ("Search text", "")
        };
        self.session.update(cx, |s, cx| {
            s.prompt(
                title,
                hint,
                move |q, cx| {
                    let from = session.read(cx).cursor;
                    let fut = if bytes {
                        futures::future::Either::Left(session.read(cx).call(FindBytes {
                            ea: from,
                            pattern: q.clone(),
                            forward: true,
                        }))
                    } else {
                        futures::future::Either::Right(session.read(cx).call(FindText {
                            ea: from,
                            text: q.clone(),
                            forward: true,
                        }))
                    };
                    let session = session.clone();
                    cx.spawn(async move |cx| {
                        let r = fut.await;
                        session.update(cx, |s, cx| match r {
                            Ok(Some(ea)) => s.navigate(ea, cx),
                            Ok(None) => s.info_msg(format!("'{q}' not found below the cursor"), cx),
                            Err(e) => s.error(e.to_string(), cx),
                        });
                    })
                    .detach();
                },
                cx,
            )
        });
    }

    // ---- rendering the main area ------------------------------------------

    /// The pane in the editor area for `mode`, with a header when split.
    fn pane(&self, mode: Mode, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = Theme::get(cx);
        let s = self.session.read(cx);
        let cursor = s.cursor;
        let segment = self
            .focus_ctx
            .read(cx)
            .addr
            .as_ref()
            .and_then(|a| a.segment.clone())
            .unwrap_or_default();
        let (title, note, view): (&'static str, String, gpui::AnyElement) = match mode {
            Mode::Linear => (
                "Linear",
                format!("{segment}  0x{cursor:x}"),
                self.listing.clone().into_any_element(),
            ),
            Mode::Graph => (
                "Graph",
                format!("{segment}  0x{cursor:x}"),
                self.graph.clone().into_any_element(),
            ),
            Mode::Hex => (
                "Hex",
                format!("{segment}  0x{cursor:x}"),
                self.hex.clone().into_any_element(),
            ),
            Mode::Pseudo => {
                let line = s
                    .caret
                    .line
                    .filter(|_| s.caret.origin == Some(crate::session::Origin::Pseudocode));
                let note = match line {
                    Some(l) => format!("Hex-Rays  ·  line {l}"),
                    None => "Hex-Rays".to_owned(),
                };
                ("Pseudocode", note, self.pseudo.clone().into_any_element())
            }
        };
        let header = self.split.then(|| self.pane_header(title, note, &t));
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .children(header)
            .child(div().flex_1().min_h_0().child(view))
            .into_any_element()
    }

    fn editor_area(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = *theme;
        if let Some(id) = self.doc
            && let Some(d) = self.docs.iter().find(|d| d.id == id)
        {
            return div()
                .flex_1()
                .min_h_0()
                .child(d.view.any())
                .into_any_element();
        }
        let panes: Vec<gpui::AnyElement> = if !self.split {
            vec![self.pane(self.mode, cx)]
        } else if self.mode == Mode::Pseudo {
            vec![self.pane(Mode::Linear, cx), self.pane(Mode::Pseudo, cx)]
        } else {
            vec![self.pane(self.mode, cx), self.pane(Mode::Pseudo, cx)]
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .children(panes.into_iter().enumerate().map(|(i, p)| {
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .when(i > 0, |d| d.border_l_1().border_color(hsla(t.border)))
                    .child(p)
            }))
            .into_any_element()
    }

    fn render_main(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *theme;
        div()
            .size_full()
            .flex()
            .child(self.rail(&t, cx))
            .when(self.show_sidebar, |d| {
                d.child(
                    div()
                        .relative()
                        .w(px(self.sidebar_w))
                        .flex_none()
                        .h_full()
                        .border_r_1()
                        .border_color(hsla(t.border))
                        .child(self.sidebar.clone())
                        .child(self.resize_handle("resize-sidebar", Resize::Sidebar, cx)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.tab_strip(&t, cx))
                    .when(self.doc.is_none(), |d| d.child(self.jump_bar(&t, cx)))
                    .child(self.editor_area(&t, cx))
                    .when(self.show_bottom, |d| {
                        d.child(
                            div()
                                .relative()
                                .h(px(self.bottom_h))
                                .flex_none()
                                .border_t_1()
                                .border_color(hsla(t.border))
                                .child(self.bottom.clone())
                                .child(self.resize_handle("resize-bottom", Resize::Bottom, cx)),
                        )
                    }),
            )
            .when(self.show_inspector, |d| {
                d.child(
                    div()
                        .relative()
                        .w(px(self.inspector_w))
                        .flex_none()
                        .h_full()
                        .border_l_1()
                        .border_color(hsla(t.border))
                        .child(self.inspector.clone())
                        .child(self.resize_handle("resize-inspector", Resize::Inspector, cx)),
                )
            })
    }

    /// Follows the mouse while a panel edge is dragged.
    fn drag_resize(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(which) = self.resizing else { return };
        let size = window.viewport_size();
        let (x, y) = (f32::from(position.x), f32::from(position.y));
        match which {
            Resize::Sidebar => self.sidebar_w = (x - RAIL_W).clamp(200., 480.),
            Resize::Inspector => self.inspector_w = (f32::from(size.width) - x).clamp(240., 480.),
            Resize::Bottom => {
                let body =
                    f32::from(size.height) - TITLE_H - BAR_HEIGHT - chrome::TOOLBAR_H - STATUS_H;
                self.bottom_h =
                    (f32::from(size.height) - STATUS_H - y).clamp(96., (body * 0.6).max(96.));
            }
        }
        cx.notify();
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = theme;
        let state = self.view_state(cx);
        let ready = self.session.read(cx).is_ready();
        let dbs: Vec<DbEntry> = self
            .tabs(cx)
            .into_iter()
            .map(|i| DbEntry {
                id: self.dbs[i].id,
                name: self.dbs[i].title(cx).into(),
                active: i == self.active,
            })
            .collect();
        self.menubar.update(cx, |m, _| {
            m.view = state;
            m.ready = ready;
            m.dbs = dbs;
        });
        self.apply_listing_options(cx);
        let phase_ready = ready;
        let loading = matches!(self.session.read(cx).phase, Phase::Loading { .. });
        let body = if phase_ready {
            self.render_main(&t, cx).into_any_element()
        } else if loading {
            self.loading(&t, cx)
        } else {
            self.welcome(&t, window, cx)
        };
        let menu_open = self.menubar.read(cx).is_open();
        let title = self.title_strip(window, &t, cx);
        let menu_row = self.menu_row(&t);
        let toolbar = self.toolbar(&t, cx);
        let status = self.status_bar(&t, cx);
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(hsla(t.bg))
            .text_color(hsla(t.text))
            .font_family(UI_FONT)
            .text_size(px(12.))
            .line_height(px(16.))
            .on_mouse_move(cx.listener(|w, ev: &gpui::MouseMoveEvent, window, cx| {
                if w.resizing.is_some() {
                    if ev.pressed_button == Some(gpui::MouseButton::Left) {
                        w.drag_resize(ev.position, window, cx);
                    } else {
                        w.resizing = None;
                        w.save_layout(cx);
                    }
                }
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|w, _, _, cx| {
                    if w.resizing.take().is_some() {
                        w.save_layout(cx);
                    }
                }),
            )
            .on_drop(cx.listener(|w, paths: &ExternalPaths, window, cx| {
                for p in paths.0.iter() {
                    w.open_path(p.clone(), window, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &GoTo, window, cx| w.goto(window, cx)))
            .on_action(cx.listener(|w, _: &CommandPalette, window, cx| w.palette(window, cx)))
            .on_action(
                cx.listener(|w, _: &TogglePseudocode, window, cx| w.toggle_pseudo(window, cx)),
            )
            .on_action(cx.listener(|w, _: &ToggleSplit, window, cx| w.toggle_split(window, cx)))
            .on_action(
                cx.listener(|w, _: &ShowPseudocode, window, cx| {
                    w.set_mode(Mode::Pseudo, window, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &ToggleGraph, window, cx| {
                let next = if w.mode == Mode::Graph && w.doc.is_none() {
                    Mode::Linear
                } else {
                    Mode::Graph
                };
                w.set_mode(next, window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &ShowGraph, window, cx| w.set_mode(Mode::Graph, window, cx)),
            )
            .on_action(cx.listener(|w, _: &ShowHex, window, cx| w.set_mode(Mode::Hex, window, cx)))
            .on_action(
                cx.listener(|w, _: &ShowListing, window, cx| w.set_mode(Mode::Linear, window, cx)),
            )
            .on_action(cx.listener(|w, _: &FocusSidebar, window, cx| {
                w.show_sidebar = true;
                w.sidebar.update(cx, |s, cx| s.focus_filter(window, cx));
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &ToggleSidebar, _, cx| w.toggle_sidebar(cx)))
            .on_action(cx.listener(|w, _: &ToggleInspector, _, cx| w.toggle_inspector(cx)))
            .on_action(cx.listener(|w, _: &ToggleBottomPanel, _, cx| w.toggle_bottom(cx)))
            .on_action(
                cx.listener(|w, _: &ToggleOutput, _, cx| {
                    w.toggle_bottom_tab(BottomTab::Output, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &ShowXrefsPanel, _, cx| {
                w.show_bottom = true;
                w.bottom.update(cx, |b, cx| {
                    b.tab = BottomTab::Xrefs;
                    cx.notify();
                });
                w.save_layout(cx);
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &ShowPythonConsole, window, cx| {
                if w.session.read(cx).is_ready() {
                    w.show_python(window, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &RunScriptFile, window, cx| w.script_dialog(window, cx)))
            .on_action(cx.listener(|w, _: &DebugStart, _, cx| {
                w.session.update(cx, |s, cx| s.dbg_start_or_continue(cx))
            }))
            .on_action(
                cx.listener(|w, _: &DebugPause, _, cx| {
                    w.session.update(cx, |s, cx| s.dbg_pause(cx))
                }),
            )
            .on_action(cx.listener(|w, _: &DebugTerminate, _, cx| {
                w.session.update(cx, |s, cx| s.dbg_terminate(cx))
            }))
            .on_action(cx.listener(|w, _: &DebugDetach, _, cx| {
                w.session.update(cx, |s, cx| s.dbg_detach(cx))
            }))
            .on_action(cx.listener(|w, _: &DebugAttach, window, cx| w.attach_picker(window, cx)))
            .on_action(cx.listener(|w, _: &DebugStepInto, _, cx| {
                w.session
                    .update(cx, |s, cx| s.dbg_resume(ResumeHow::StepInto, 0, cx))
            }))
            .on_action(cx.listener(|w, _: &DebugStepOver, _, cx| {
                w.session
                    .update(cx, |s, cx| s.dbg_resume(ResumeHow::StepOver, 0, cx))
            }))
            .on_action(cx.listener(|w, _: &DebugStepOut, _, cx| {
                w.session
                    .update(cx, |s, cx| s.dbg_resume(ResumeHow::StepOut, 0, cx))
            }))
            .on_action(cx.listener(|w, _: &DebugRunToCursor, _, cx| {
                let ea = w.session.read(cx).cursor;
                w.session
                    .update(cx, |s, cx| s.dbg_resume(ResumeHow::RunTo, ea, cx))
            }))
            .on_action(cx.listener(|w, _: &ToggleBreakpoint, _, cx| {
                let ea = w.session.read(cx).cursor;
                w.session.update(cx, |s, cx| s.dbg_toggle_bpt(ea, cx))
            }))
            .on_action(cx.listener(|w, _: &DebuggerOptions, window, cx| {
                w.open_settings(Section::Debugger, window, cx)
            }))
            .on_action(cx.listener(|w, _: &ShowBreakpoints, _, cx| {
                w.show_debug_tab(BottomTab::Breakpoints, cx)
            }))
            .on_action(
                cx.listener(|w, _: &ShowRegisters, _, cx| {
                    w.show_debug_tab(BottomTab::Registers, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &ShowStack, _, cx| w.show_debug_tab(BottomTab::Stack, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowMemory, _, cx| w.show_debug_tab(BottomTab::Memory, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowCallStack, _, cx| {
                    w.show_debug_tab(BottomTab::CallStack, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &ShowThreads, _, cx| w.show_debug_tab(BottomTab::Threads, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowModules, _, cx| w.show_debug_tab(BottomTab::Modules, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowDebugEvents, _, cx| {
                    w.show_debug_tab(BottomTab::Events, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &ShowExceptions, _, cx| {
                w.show_debug_tab(BottomTab::Exceptions, cx)
            }))
            .on_action(cx.listener(|w, _: &ShowFunctions, window, cx| {
                w.show_tab(Tab::Functions, window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &ShowNames, window, cx| w.show_tab(Tab::Names, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowStrings, window, cx| w.show_tab(Tab::Strings, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowImports, window, cx| w.show_tab(Tab::Imports, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowExports, window, cx| w.show_tab(Tab::Exports, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowSegments, window, cx| {
                    w.show_tab(Tab::Segments, window, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &ShowTypes, window, cx| w.show_tab(Tab::Types, window, cx)),
            )
            .on_action(cx.listener(|w, _: &ShowLocalTypes, window, cx| {
                w.open_doc(DocKey::LocalTypes(None), window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &ShowStructures, window, cx| w.pick_type(false, window, cx)),
            )
            .on_action(cx.listener(|w, _: &ShowEnums, window, cx| w.pick_type(true, window, cx)))
            .on_action(cx.listener(|w, _: &ShowFrame, window, cx| {
                let s = w.session.read(cx);
                match s.function_at(s.cursor).map(|f| f.start) {
                    Some(start) => w.open_doc(DocKey::Frame(start), window, cx),
                    None => w.session.update(cx, |s, cx| {
                        s.info_msg("The cursor is not inside a function", cx)
                    }),
                }
            }))
            .on_action(cx.listener(|w, _: &ShowSignatures, window, cx| {
                w.open_doc(DocKey::Signatures, window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &ShowPlugins, window, cx| {
                    w.open_doc(DocKey::Plugins, window, cx)
                }),
            )
            .on_action(cx.listener(|w, a: &RunPlugin, window, cx| w.run_plugin(a, window, cx)))
            .on_action(cx.listener(|w, _: &ShowProximity, window, cx| {
                let s = w.session.read(cx);
                match s.function_at(s.cursor).map(|f| f.start) {
                    Some(start) => w.open_doc(DocKey::Proximity(start), window, cx),
                    None => w.session.update(cx, |s, cx| {
                        s.info_msg("The cursor is not inside a function", cx)
                    }),
                }
            }))
            .on_action(cx.listener(|w, _: &LuminaPullAll, window, cx| {
                w.lumina(LuminaAction::PullAll, window, cx)
            }))
            .on_action(cx.listener(|w, _: &LuminaPullFunction, window, cx| {
                w.lumina(LuminaAction::PullFunc, window, cx)
            }))
            .on_action(cx.listener(|w, _: &LuminaPushAll, window, cx| {
                w.lumina(LuminaAction::PushAll, window, cx)
            }))
            .on_action(cx.listener(|w, _: &LuminaPushFunction, window, cx| {
                w.lumina(LuminaAction::PushFunc, window, cx)
            }))
            .on_action(cx.listener(|w, _: &CloseTab, window, cx| {
                if let Some(id) = w.doc {
                    w.close_doc(id, window, cx);
                }
            }))
            .on_action(cx.listener(|w, _: &Back, _, cx| w.session.update(cx, |s, cx| s.back(cx))))
            .on_action(
                cx.listener(|w, _: &Forward, _, cx| w.session.update(cx, |s, cx| s.forward(cx))),
            )
            .on_action(cx.listener(|w, _: &OpenFile, window, cx| w.prompt_open(window, cx)))
            .on_action(cx.listener(|w, _: &OpenWithOptions, window, cx| {
                w.open_options_dialog(None, window, cx)
            }))
            .on_action(cx.listener(|w, _: &CloseDatabase, window, cx| {
                let id = w.id;
                w.close_db_checked(id, window, cx);
            }))
            .on_action(cx.listener(|w, _: &NextDatabase, window, cx| w.cycle_db(1, window, cx)))
            .on_action(cx.listener(|w, _: &PrevDatabase, window, cx| w.cycle_db(-1, window, cx)))
            .on_action(
                cx.listener(|w, _: &SwitchDatabase1, window, cx| w.activate_nth(0, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase2, window, cx| w.activate_nth(1, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase3, window, cx| w.activate_nth(2, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase4, window, cx| w.activate_nth(3, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase5, window, cx| w.activate_nth(4, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase6, window, cx| w.activate_nth(5, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase7, window, cx| w.activate_nth(6, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase8, window, cx| w.activate_nth(7, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &SwitchDatabase9, window, cx| w.activate_nth(8, window, cx)),
            )
            .on_action(cx.listener(|w, _: &ToggleMinimap, _, cx| {
                w.graph.update(cx, |g, cx| g.toggle_minimap(cx))
            }))
            .on_action(cx.listener(|w, _: &Save, _, cx| w.session.update(cx, |s, cx| s.save(cx))))
            .on_action(
                cx.listener(|w, _: &Undo, _, cx| w.session.update(cx, |s, cx| s.undo(false, cx))),
            )
            .on_action(
                cx.listener(|w, _: &Redo, _, cx| w.session.update(cx, |s, cx| s.undo(true, cx))),
            )
            .on_action(cx.listener(|w, _: &SearchBytes, _, cx| w.search(true, cx)))
            .on_action(cx.listener(|w, _: &SearchText, _, cx| w.search(false, cx)))
            .on_action(cx.listener(|w, _: &OpenSettings, window, cx| {
                w.open_settings(Section::Appearance, window, cx)
            }))
            .on_action(cx.listener(|w, _: &OpenKeymap, window, cx| {
                w.open_settings(Section::Keyboard, window, cx)
            }))
            .on_action(cx.listener(|w, _: &ShowShortcuts, window, cx| {
                let dialog = cx.new(ShortcutsDialog::new);
                w.show_dialog(dialog, window, cx);
            }))
            .on_action(cx.listener(|w, _: &About, window, cx| {
                let session = w.session.clone();
                let dialog = cx.new(|cx| AboutDialog::new(&session, cx));
                w.show_dialog(dialog, window, cx);
            }))
            .on_action(cx.listener(|w, _: &FocusMenuBar, window, cx| {
                w.menubar.update(cx, |m, cx| m.focus_bar(window, cx))
            }))
            .on_action(cx.listener(|w, _: &MenuFile, window, cx| w.open_menu(0, window, cx)))
            .on_action(cx.listener(|w, _: &MenuEdit, window, cx| w.open_menu(1, window, cx)))
            .on_action(cx.listener(|w, _: &MenuJump, window, cx| w.open_menu(2, window, cx)))
            .on_action(cx.listener(|w, _: &MenuSearch, window, cx| w.open_menu(3, window, cx)))
            .on_action(cx.listener(|w, _: &MenuView, window, cx| w.open_menu(4, window, cx)))
            .on_action(cx.listener(|w, _: &MenuDebugger, window, cx| w.open_menu(5, window, cx)))
            .on_action(cx.listener(|w, _: &MenuOptions, window, cx| w.open_menu(6, window, cx)))
            .on_action(cx.listener(|w, _: &MenuWindows, window, cx| w.open_menu(7, window, cx)))
            .on_action(cx.listener(|w, _: &MenuHelp, window, cx| w.open_menu(8, window, cx)))
            .on_action(cx.listener(|w, _: &CopyAddress, _, cx| {
                let ea = w.session.read(cx).cursor;
                zui_editor::clipboard::write(format!("{ea:X}"), cx);
                w.session
                    .update(cx, |s, cx| s.info_msg(format!("Copied {ea:X}"), cx));
            }))
            .on_action(cx.listener(|w, _: &Quit, window, cx| {
                if w.should_close(window, cx) {
                    window.remove_window();
                }
            }))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(menu_row)
                    .child(toolbar)
                    .child(div().flex_1().min_h_0().child(body))
                    .child(status),
            )
            // While a menu is open, a clear sheet under the bar swallows the
            // click that dismisses it.
            .when(menu_open, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(TITLE_H + BAR_HEIGHT))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .occlude()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|w, _, window, cx| {
                                w.menubar.update(cx, |m, cx| m.close(window, cx))
                            }),
                        ),
                )
            })
            .when_some(self.modal.as_ref(), |d, m| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(hsla_a(t.scrim))
                        .flex()
                        .justify_center()
                        .pt(px(96.))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|w, _, window, cx| w.close_modal(window, cx)),
                        )
                        .child(
                            div()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(m.view.clone()),
                        ),
                )
            })
            .when_some(self.dialog.as_ref(), |d, dialog| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(hsla_a(t.scrim))
                        .flex()
                        .items_center()
                        .justify_center()
                        .py(px(40.))
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|w, _, window, cx| {
                                // Clicking outside a question does not answer it.
                                if w.pending_answer.is_none() {
                                    w.close_dialog(window, cx);
                                }
                            }),
                        )
                        .child(
                            div()
                                .max_h_full()
                                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation()
                                })
                                .child(dialog.view.clone()),
                        ),
                )
            })
    }
}

/// The shortcut currently bound to action `A`, for labels.
fn shortcut<A: gpui::Action + Default>(cx: &App) -> String {
    crate::actions::shortcut_text(&A::default(), cx)
}
