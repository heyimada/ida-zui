// One open database as the workspace holds it: its session, every view of it
// and the state that belongs to the tab (mode, editor tabs, jump choices).
// The workspace keeps a list of these and shows the active one; menu bar,
// settings, keymap, palette and panel sizes stay global.
use crate::bottom::BottomPanel;
use crate::docs::{DocKey, DocView};
use crate::focus::Focus;
use crate::graph::GraphView;
use crate::hex::HexView;
use crate::inspector::Inspector;
use crate::listing::ListingView;
use crate::modal::Choice;
use crate::pseudo::PseudoView;
use crate::session::{Phase, Session};
use crate::sidebar::Sidebar;
use gpui::{App, Entity, SharedString, Subscription};
use std::rc::Rc;

/// What the centre shows when no editor tab is selected.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Linear,
    Graph,
    Hex,
    Pseudo,
}

/// An editor tab beyond the main view.
pub struct DocTab {
    pub id: u64,
    pub key: DocKey,
    pub title: SharedString,
    pub view: DocView,
    pub _sub: Subscription,
}

/// How a database tab looks in the strip.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    /// Nothing open (the welcome screen).
    Empty,
    Loading,
    Ready,
    Failed,
}

pub struct Database {
    pub id: u64,
    pub session: Entity<Session>,
    pub listing: Entity<ListingView>,
    pub graph: Entity<GraphView>,
    pub hex: Entity<HexView>,
    pub pseudo: Entity<PseudoView>,
    pub mode: Mode,
    /// The mode Tab returns to from pseudocode (see `tab_toggle`).
    pub tab_back: Option<Mode>,
    /// The editor tab in front, when it is not the main view.
    pub doc: Option<u64>,
    pub docs: Vec<DocTab>,
    pub next_doc: u64,
    pub sidebar: Entity<Sidebar>,
    pub focus_ctx: Entity<Focus>,
    pub inspector: Entity<Inspector>,
    pub bottom: Entity<BottomPanel>,
    pub goto_choices: Option<Rc<Vec<Choice>>>,
    /// The last search (`true` for bytes) and its text, for "search next".
    pub last_search: Option<(bool, String)>,
    /// Jump here once the database finishes opening (CLI `--goto`).
    pub pending_goto: Option<String>,
    /// What the tab strip last showed for this database; an inactive tab
    /// repaints the window only when this changes.
    pub chip: (TabKind, bool, u32),
    pub _subs: Vec<Subscription>,
}

impl Database {
    pub fn kind(&self, cx: &App) -> TabKind {
        match self.session.read(cx).phase {
            Phase::Empty => TabKind::Empty,
            Phase::Loading { .. } => TabKind::Loading,
            Phase::Ready => TabKind::Ready,
            Phase::Failed(_) => TabKind::Failed,
        }
    }

    /// The tab's signature: its state, whether it has unsaved changes and
    /// the analysis percentage it shows.
    pub fn signature(&self, cx: &App) -> (TabKind, bool, u32) {
        let s = self.session.read(cx);
        (
            self.kind(cx),
            s.dirty,
            s.analysis.as_ref().map_or(0, |a| a.percent),
        )
    }

    /// The file name for the tab: the database's input, or the file being opened.
    pub fn title(&self, cx: &App) -> String {
        let s = self.session.read(cx);
        s.file_name()
            .or_else(|| match &s.phase {
                Phase::Loading { path, .. } => {
                    path.file_name().map(|n| n.to_string_lossy().into_owned())
                }
                _ => s
                    .request
                    .as_ref()
                    .and_then(|r| r.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned()),
            })
            .unwrap_or_default()
    }

    pub fn dirty(&self, cx: &App) -> bool {
        self.session.read(cx).dirty
    }
}

/// Tab / F5 without a split: the mode to show next and the mode Tab returns
/// to. Pseudocode is entered from any other mode and left for the one it came
/// from (the listing when it was entered some other way).
pub fn tab_toggle(mode: Mode, back: Option<Mode>) -> (Mode, Option<Mode>) {
    match mode {
        Mode::Pseudo => (back.unwrap_or(Mode::Linear), None),
        from => (Mode::Pseudo, Some(from)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_peeks_at_pseudocode_and_returns_to_the_view_it_left() {
        for from in [Mode::Linear, Mode::Graph, Mode::Hex] {
            let (peek, back) = tab_toggle(from, None);
            assert!(peek == Mode::Pseudo);
            let (home, back) = tab_toggle(peek, back);
            assert!(home == from);
            assert!(back.is_none());
        }
    }

    #[test]
    fn tab_from_pseudocode_chosen_elsewhere_goes_to_the_listing() {
        let (home, back) = tab_toggle(Mode::Pseudo, None);
        assert!(home == Mode::Linear);
        assert!(back.is_none());
    }
}
