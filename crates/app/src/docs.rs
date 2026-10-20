// What the centre tabs beyond Disassembly / Graph / Hex are made of: a key
// that says which editor a tab holds, and the events an editor view sends up
// to the workspace that owns its tab.
use gpui::SharedString;
use ida_proto::Ea;

/// Which editor a tab holds; at most one tab exists per key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocKey {
    /// The local types browser, optionally with a type selected.
    LocalTypes(Option<String>),
    Struct(String),
    Enum(String),
    /// The stack frame of the function starting at the address.
    Frame(Ea),
    /// The FLIRT signatures window.
    Signatures,
    /// The plugins window.
    Plugins,
    /// The call neighbourhood of the function starting at the address.
    Proximity(Ea),
    /// A code view opened in a tab of its own (unique per tab).
    Code(u64),
}

impl DocKey {
    /// True if `other` would open in the tab that `self` already is.
    pub fn same_tab(&self, other: &DocKey) -> bool {
        match (self, other) {
            (DocKey::LocalTypes(_), DocKey::LocalTypes(_)) => true,
            _ => self == other,
        }
    }
}

pub enum DocEvent {
    /// The tab's title changed (a type was renamed, say).
    Title(SharedString),
    /// Open another editor, e.g. from a list.
    Open(DocKey),
    /// Show this address in the listing (and leave the editor tab).
    Navigate(Ea),
}

/// The view inside a tab.
pub enum DocView {
    Types(gpui::Entity<crate::types_view::TypesView>),
    Struct(gpui::Entity<crate::struct_view::StructView>),
    Enum(gpui::Entity<crate::enum_view::EnumView>),
    Frame(gpui::Entity<crate::frame_view::FrameView>),
    Sigs(gpui::Entity<crate::sigs_view::SigsView>),
    Plugins(gpui::Entity<crate::plugins_view::PluginsView>),
    Proximity(gpui::Entity<crate::proximity_view::ProximityView>),
    Code(CodeView),
}

/// A listing, graph, hex view or pseudocode in a tab of its own.
#[derive(Clone)]
pub enum CodeView {
    Listing(gpui::Entity<crate::listing::ListingView>),
    Graph(gpui::Entity<crate::graph::GraphView>),
    Hex(gpui::Entity<crate::hex::HexView>),
    Pseudo(gpui::Entity<crate::pseudo::PseudoView>),
}

impl CodeView {
    pub fn kind(&self) -> crate::session::ViewKind {
        use crate::session::ViewKind as K;
        match self {
            CodeView::Listing(_) => K::Listing,
            CodeView::Graph(_) => K::Graph,
            CodeView::Hex(_) => K::Hex,
            CodeView::Pseudo(_) => K::Pseudocode,
        }
    }

    /// Makes a view of `kind` over `session`, showing `ea`, pinned or not.
    pub fn open(
        kind: crate::session::ViewKind,
        session: &gpui::Entity<crate::session::Session>,
        ea: Ea,
        pinned: bool,
        cx: &mut gpui::App,
    ) -> Self {
        use crate::session::ViewKind as K;
        use gpui::AppContext;
        match kind {
            K::Listing => {
                let v = cx.new(|cx| crate::listing::ListingView::new(session.clone(), cx));
                v.update(cx, |v, cx| {
                    v.pinned = pinned;
                    v.jump_to(ea, cx)
                });
                CodeView::Listing(v)
            }
            K::Graph => {
                let v = cx.new(|cx| crate::graph::GraphView::new(session.clone(), cx));
                v.update(cx, |v, _| {
                    v.pinned = pinned;
                    v.set_wanted(ea)
                });
                CodeView::Graph(v)
            }
            K::Hex => {
                let v = cx.new(|cx| crate::hex::HexView::new(session.clone(), cx));
                v.update(cx, |v, cx| {
                    v.pinned = pinned;
                    v.jump_to(ea, cx)
                });
                CodeView::Hex(v)
            }
            K::Pseudocode => {
                let v = cx.new(|cx| crate::pseudo::PseudoView::new(session.clone(), cx));
                v.update(cx, |v, _| {
                    v.pinned = pinned;
                    v.set_wanted(ea)
                });
                CodeView::Pseudo(v)
            }
        }
    }

    pub fn origin(&self, cx: &gpui::App) -> crate::session::Origin {
        match self {
            CodeView::Listing(v) => v.read(cx).origin(),
            CodeView::Graph(v) => v.read(cx).origin(),
            CodeView::Hex(v) => v.read(cx).origin(),
            CodeView::Pseudo(v) => v.read(cx).origin(),
        }
    }

    pub fn pinned(&self, cx: &gpui::App) -> bool {
        match self {
            CodeView::Listing(v) => v.read(cx).pinned,
            CodeView::Graph(v) => v.read(cx).pinned,
            CodeView::Hex(v) => v.read(cx).pinned,
            CodeView::Pseudo(v) => v.read(cx).pinned,
        }
    }

    pub fn current_ea(&self, cx: &gpui::App) -> Option<Ea> {
        match self {
            CodeView::Listing(v) => v.read(cx).current_ea(),
            CodeView::Graph(v) => v.read(cx).current_ea(),
            CodeView::Hex(v) => v.read(cx).current_ea(),
            CodeView::Pseudo(v) => v.read(cx).current_ea(),
        }
    }

    /// Shown (rendered) or not: the decompiler and the graph only work while shown.
    pub fn set_visible(&self, visible: bool, cx: &mut gpui::App) {
        match self {
            CodeView::Listing(_) => {}
            CodeView::Graph(v) => v.update(cx, |g, cx| g.set_visible(visible, cx)),
            CodeView::Hex(v) => v.update(cx, |h, _| h.visible = visible),
            CodeView::Pseudo(v) => v.update(cx, |p, cx| p.set_visible(visible, cx)),
        }
    }
}

impl DocView {
    pub fn any(&self) -> gpui::AnyView {
        match self {
            DocView::Types(v) => v.clone().into(),
            DocView::Struct(v) => v.clone().into(),
            DocView::Enum(v) => v.clone().into(),
            DocView::Frame(v) => v.clone().into(),
            DocView::Sigs(v) => v.clone().into(),
            DocView::Plugins(v) => v.clone().into(),
            DocView::Proximity(v) => v.clone().into(),
            DocView::Code(CodeView::Listing(v)) => v.clone().into(),
            DocView::Code(CodeView::Graph(v)) => v.clone().into(),
            DocView::Code(CodeView::Hex(v)) => v.clone().into(),
            DocView::Code(CodeView::Pseudo(v)) => v.clone().into(),
        }
    }

    pub fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        use gpui::Focusable;
        match self {
            DocView::Types(v) => v.read(cx).focus_handle(cx),
            DocView::Struct(v) => v.read(cx).focus_handle(cx),
            DocView::Enum(v) => v.read(cx).focus_handle(cx),
            DocView::Frame(v) => v.read(cx).focus_handle(cx),
            DocView::Sigs(v) => v.read(cx).focus_handle(cx),
            DocView::Plugins(v) => v.read(cx).focus_handle(cx),
            DocView::Proximity(v) => v.read(cx).focus_handle(cx),
            DocView::Code(CodeView::Listing(v)) => v.read(cx).focus_handle(cx),
            DocView::Code(CodeView::Graph(v)) => v.read(cx).focus_handle(cx),
            DocView::Code(CodeView::Hex(v)) => v.read(cx).focus_handle(cx),
            DocView::Code(CodeView::Pseudo(v)) => v.read(cx).focus_handle(cx),
        }
    }
}
