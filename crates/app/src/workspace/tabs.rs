// Code views as tabs: a listing, graph, hex view or pseudocode opened in a tab
// of its own ("Open in new tab"), next to the main view. A tab opened this way
// is pinned: it keeps its own place and only moves on its own jumps (see
// `session::follows`). The Linear / Graph / Hex / Pseudocode control
// retargets whichever tab is in front.
use super::Workspace;
use crate::database::{DocTab, Mode};
use crate::docs::{CodeView, DocKey, DocView};
use crate::session::{Driver, ViewKind};
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

impl Workspace {
    /// The code view in front when a code tab is, with its tab id.
    pub(super) fn front_code(&self) -> Option<(u64, CodeView)> {
        let id = self.doc?;
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
        self.doc.is_none().then_some(match self.mode {
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
        self.show_doc(id, window, cx);
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
        let front = db.doc.and_then(|id| db.docs.iter().find(|d| d.id == id));
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
                cv.set_visible(db.doc == Some(d.id), cx);
            }
        }
    }
}
