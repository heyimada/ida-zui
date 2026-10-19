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
        }
    }
}
