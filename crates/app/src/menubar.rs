// The in-app menu bar. gpui on Windows has no native app menus, so this is a
// row of titles with dropdowns that behave like a desktop menu: click to open,
// hover to switch while one is open, arrows/Enter/Escape, Alt or F10 to focus,
// Alt+letter to open. Labels, shortcuts and enablement come from the command
// catalog and the live keymap, never from strings kept here.
use crate::actions::{self, Cancel, Confirm, MenuLeft, MenuRight, SelectNext, SelectPrev};
use crate::scrollbar::{self, Axes, Source};
use crate::settings;
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, icon};
use gpui::{
    Action, App, Context, EventEmitter, FocusHandle, Focusable, HighlightStyle, KeyDownEvent,
    MouseButton, Pixels, Point, ScrollHandle, SharedString, StyledText, Subscription,
    UnderlineStyle, Window, anchored, deferred, div, prelude::*, px, relative,
};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

pub const BAR_HEIGHT: f32 = 28.;
const ROW_HEIGHT: f32 = 26.;
const CODE_SIZES: [f32; 8] = [10., 11., 12., 13., 14., 16., 18., 20.];

/// Which panels and views are showing, so View menu entries can be checked.
#[derive(Clone, Copy, Default)]
pub struct ViewState {
    pub sidebar: bool,
    pub inspector: bool,
    pub bottom: bool,
    pub split: bool,
    /// Pseudocode is shown without casts (this database).
    pub hide_casts: bool,
    /// A process exists (running or suspended).
    pub debug_active: bool,
    pub debug_suspended: bool,
}

/// What activating an item does.
pub enum Target {
    Action(Box<dyn Action>),
    Theme(&'static str),
    CodeFontSize(f32),
    Recent(String),
    ClearRecent,
    /// Bring the database tab with this id forward.
    Database(u64),
    /// Something a context menu entry does with data it captured (jump to a
    /// row's address, copy its name).
    Run(Rc<dyn Fn(&mut Window, &mut App)>),
}

impl Clone for Target {
    fn clone(&self) -> Self {
        match self {
            Self::Action(a) => Self::Action(a.boxed_clone()),
            Self::Theme(t) => Self::Theme(t),
            Self::CodeFontSize(s) => Self::CodeFontSize(*s),
            Self::Recent(p) => Self::Recent(p.clone()),
            Self::ClearRecent => Self::ClearRecent,
            Self::Database(id) => Self::Database(*id),
            Self::Run(f) => Self::Run(f.clone()),
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub label: SharedString,
    pub detail: SharedString,
    pub shortcut: SharedString,
    pub enabled: bool,
    pub checked: bool,
    pub target: Target,
}

#[derive(Clone)]
pub enum Entry {
    Item(Item),
    Sep,
    /// A caption (a context menu's title): never highlighted, no mnemonic.
    Header(SharedString),
    Sub {
        label: SharedString,
        enabled: bool,
        entries: Vec<Entry>,
    },
}

/// A menu label as written: `&` marks the mnemonic letter that follows it
/// (`Make &data`), `&&` is a literal ampersand. Returns the text to show and
/// the byte offset of the marked letter in it.
pub fn parse_label(raw: &str) -> (String, Option<usize>) {
    let mut text = String::with_capacity(raw.len());
    let mut mark = None;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            text.push(c);
        } else if chars.peek() == Some(&'&') {
            chars.next();
            text.push('&');
        } else if mark.is_none() && chars.peek().is_some() {
            mark = Some(text.len());
        }
    }
    (text, mark)
}

fn entry_label(e: &Entry) -> Option<&SharedString> {
    match e {
        Entry::Item(i) => Some(&i.label),
        Entry::Sub { label, .. } => Some(label),
        Entry::Sep | Entry::Header(_) => None,
    }
}

/// The mnemonic of each entry, as a byte offset into its displayed label
/// (see `parse_label`): the letter marked with `&`, else the first letter
/// that no other entry of the same menu level took.
pub fn mnemonics(entries: &[Entry]) -> Vec<Option<usize>> {
    let parsed: Vec<Option<(String, Option<usize>)>> = entries
        .iter()
        .map(|e| entry_label(e).map(|l| parse_label(l)))
        .collect();
    let letter = |text: &str, at: usize| text[at..].chars().next().map(|c| c.to_ascii_lowercase());
    // Marked letters are claimed first, so an earlier unmarked label cannot take them.
    let mut taken: Vec<char> = parsed
        .iter()
        .flatten()
        .filter_map(|(t, m)| m.and_then(|at| letter(t, at)))
        .collect();
    parsed
        .iter()
        .map(|p| {
            let (text, mark) = p.as_ref()?;
            if mark.is_some() {
                return *mark;
            }
            let (at, ch) = text
                .char_indices()
                .find(|(_, c)| c.is_alphanumeric() && !taken.contains(&c.to_ascii_lowercase()))?;
            taken.push(ch.to_ascii_lowercase());
            Some(at)
        })
        .collect()
}

impl Entry {
    fn selectable(&self) -> bool {
        match self {
            Entry::Item(i) => i.enabled,
            Entry::Sub { enabled, .. } => *enabled,
            Entry::Sep | Entry::Header(_) => false,
        }
    }
}

#[derive(Clone)]
pub struct Menu {
    pub title: &'static str,
    /// Byte range of the mnemonic letter in `title`.
    pub mnemonic: Range<usize>,
    pub entries: Vec<Entry>,
}

pub enum MenuEvent {
    Invoke(Target),
}

/// One open database, for the Windows menu.
#[derive(Clone)]
pub struct DbEntry {
    pub id: u64,
    pub name: SharedString,
    pub active: bool,
}

/// A menu entry for the catalog command `name`: its label (or `label`), the
/// live shortcut, and disabled when it needs a database and there is none.
pub fn command_entry(name: &str, label: Option<&str>, ready: bool, cx: &App) -> Entry {
    let c = actions::find(name).unwrap_or_else(|| panic!("menu refers to unknown command {name}"));
    let action = (c.make)();
    Entry::Item(Item {
        label: label.unwrap_or(c.label).to_owned().into(),
        detail: SharedString::default(),
        shortcut: actions::shortcut_text(&*action, cx).into(),
        enabled: ready || !c.needs_db,
        checked: false,
        target: Target::Action(action),
    })
}

/// `entry` with a check mark while `on`.
pub fn checked(entry: Entry, on: bool) -> Entry {
    match entry {
        Entry::Item(mut i) => {
            i.checked = on;
            Entry::Item(i)
        }
        other => other,
    }
}

/// An entry that runs `f` (see `Target::Run`).
pub fn run_entry(
    label: impl Into<SharedString>,
    shortcut: impl Into<SharedString>,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Entry {
    Entry::Item(Item {
        label: label.into(),
        detail: SharedString::default(),
        shortcut: shortcut.into(),
        enabled: true,
        checked: false,
        target: Target::Run(Rc::new(f)),
    })
}

/// Builds the menus from the catalog, the live keymap and the settings.
pub fn build(
    ready: bool,
    view: ViewState,
    dbs: &[DbEntry],
    ops: &crate::operands::Ops,
    cx: &App,
) -> Vec<Menu> {
    let cmd = |name: &str, label: Option<&str>| -> Entry { command_entry(name, label, ready, cx) };
    macro_rules! c {
        ($a:ident) => {
            cmd(<actions::$a as Action>::name_for_type(), None)
        };
        ($a:ident, $label:expr) => {
            cmd(<actions::$a as Action>::name_for_type(), Some($label))
        };
    }
    // A command whose menu entry carries a check mark while `on`.
    let toggle = |entry: Entry, on: bool| match entry {
        Entry::Item(mut i) => {
            i.checked = on;
            Entry::Item(i)
        }
        other => other,
    };
    // A command that is only available in some debugger states.
    let gate = |entry: Entry, on: bool| match entry {
        Entry::Item(mut i) => {
            i.enabled = i.enabled && on;
            Entry::Item(i)
        }
        other => other,
    };
    let s = settings::get(cx);
    let recent: Vec<Entry> = s
        .recent
        .iter()
        .filter(|r| Path::new(&r.path).exists())
        .map(|r| {
            let p = Path::new(&r.path);
            Entry::Item(Item {
                label: p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
                    .into(),
                detail: p
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
                    .into(),
                shortcut: SharedString::default(),
                enabled: true,
                checked: false,
                target: Target::Recent(r.path.clone()),
            })
        })
        .collect();
    let has_recent = !recent.is_empty();
    let mut recent_entries = recent;
    if has_recent {
        recent_entries.push(Entry::Sep);
        recent_entries.push(Entry::Item(Item {
            label: "Clear list".into(),
            detail: SharedString::default(),
            shortcut: SharedString::default(),
            enabled: true,
            checked: false,
            target: Target::ClearRecent,
        }));
    }
    let themes = Theme::ALL
        .iter()
        .map(|t| {
            Entry::Item(Item {
                label: t.name.into(),
                detail: SharedString::default(),
                shortcut: SharedString::default(),
                enabled: true,
                checked: t.name == s.theme().name,
                target: Target::Theme(t.name),
            })
        })
        .collect();
    let sizes = CODE_SIZES
        .iter()
        .map(|&size| {
            Entry::Item(Item {
                label: format!("{size} pt").into(),
                detail: SharedString::default(),
                shortcut: SharedString::default(),
                enabled: true,
                checked: (s.code_font_size - size).abs() < 0.01,
                target: Target::CodeFontSize(size),
            })
        })
        .collect();
    // Edit > Plugins: the plugins IDA would list in its own menu, with their
    // live hotkeys, then the window.
    let mut plugin_entries: Vec<Entry> = crate::plugins::list()
        .iter()
        .filter(|p| crate::plugins::runnable(p))
        .map(|p| {
            let action = crate::plugins::run_action(p);
            Entry::Item(Item {
                label: if p.arg == 0 {
                    p.name.clone()
                } else {
                    format!("{} ({})", p.name, p.arg)
                }
                .into(),
                detail: SharedString::default(),
                shortcut: actions::shortcut_text(&action, cx).into(),
                enabled: ready,
                checked: false,
                target: Target::Action(Box::new(action)),
            })
        })
        .collect();
    if plugin_entries.is_empty() {
        plugin_entries.push(Entry::Item(Item {
            label: "No plugins found".into(),
            detail: SharedString::default(),
            shortcut: SharedString::default(),
            enabled: false,
            checked: false,
            target: Target::ClearRecent,
        }));
    }
    plugin_entries.push(Entry::Sep);
    plugin_entries.push(c!(ShowPlugins, "Plugins window…"));
    let m = |title: &'static str, at: usize, entries: Vec<Entry>| Menu {
        title,
        mnemonic: at..at + 1,
        entries,
    };
    vec![
        m(
            "File",
            0,
            vec![
                c!(OpenFile, "&Open…"),
                c!(OpenWithOptions, "Open &with options…"),
                c!(LocateIda, "&Locate IDA…"),
                Entry::Sub {
                    label: "&Recent files".into(),
                    enabled: has_recent,
                    entries: recent_entries,
                },
                Entry::Sep,
                c!(RunScriptFile, "Script &file…"),
                Entry::Sep,
                Entry::Sub {
                    label: "L&umina".into(),
                    enabled: true,
                    entries: vec![
                        c!(LuminaPullAll, "Pull metadata for all functions"),
                        c!(LuminaPullFunction, "Pull metadata for the current function"),
                        Entry::Sep,
                        c!(LuminaPushAll, "Push metadata for all functions…"),
                        c!(
                            LuminaPushFunction,
                            "Push metadata for the current function…"
                        ),
                    ],
                },
                Entry::Sep,
                c!(Save, "&Save database"),
                c!(CloseDatabase, "&Close database"),
                Entry::Sep,
                c!(Quit, "E&xit"),
            ],
        ),
        m(
            "Edit",
            0,
            vec![
                c!(Undo, "&Undo"),
                c!(Redo, "&Redo"),
                Entry::Sep,
                c!(Copy, "Cop&y"),
                c!(CopyWithAddresses, "Copy &with addresses"),
                c!(CopyAddress, "Copy current &address"),
                Entry::Sep,
                c!(Rename, "Re&name"),
                c!(Comment, "Co&mment"),
                c!(RepeatableComment, "Re&peatable comment"),
                c!(SetType, "Set &type"),
                Entry::Sep,
                c!(MakeCode, "Make &code"),
                c!(MakeData, "Make &data"),
                c!(MakeString, "Make &string"),
                c!(Undefine, "Unde&fine"),
                Entry::Sep,
                Entry::Sub {
                    label: "&Operand types".into(),
                    enabled: ready && ops.has_operand(),
                    entries: crate::context_menu::operand_entries(ops, cx),
                },
                c!(DefineArray),
                c!(SetStringType, "Strin&g type…"),
                Entry::Sep,
                c!(MakeFunction, "Cr&eate function"),
                c!(DeleteFunction, "De&lete function"),
                c!(EditFunctionProps),
                Entry::Sep,
                c!(ToggleCollapse, "E&xpand / collapse"),
                Entry::Sep,
                Entry::Sub {
                    label: "Patc&h program".into(),
                    enabled: ready,
                    entries: crate::context_menu::patch_entries(cx),
                },
                Entry::Sep,
                Entry::Sub {
                    label: "Plug&ins".into(),
                    enabled: ready,
                    entries: plugin_entries,
                },
            ],
        ),
        m(
            "Jump",
            0,
            vec![
                c!(GoTo, "Jump to address…"),
                c!(JumpToFunction),
                c!(JumpToSegment),
                c!(JumpToEntry),
                Entry::Sep,
                c!(Back),
                c!(Forward),
                Entry::Sep,
                c!(MarkPosition, "Mark position…"),
                c!(JumpToMark, "Jump to marked position…"),
                Entry::Sep,
                c!(PrevFunction),
                c!(NextFunction),
                Entry::Sep,
                c!(XrefsTo),
                c!(XrefsFrom),
            ],
        ),
        m(
            "Search",
            0,
            vec![
                c!(SearchText, "Text…"),
                c!(SearchNextText, "Text again"),
                c!(SearchBytes, "Bytes…"),
                c!(SearchNextBytes, "Bytes again"),
                Entry::Sep,
                c!(SearchImmediate, "Immediate value…"),
                c!(SearchNextImmediate, "Immediate value again"),
                Entry::Sep,
                c!(SearchNextCode, "Next code"),
                c!(SearchNextData, "Next data"),
                c!(SearchNextUnexplored, "Next unexplored"),
                c!(SearchNotFunction, "Next code outside a function"),
            ],
        ),
        m(
            "View",
            0,
            vec![
                c!(ShowListing, "Disassembly"),
                c!(ShowGraph, "Graph"),
                c!(ShowHex, "Hex"),
                c!(ShowPseudocode, "Pseudocode"),
                c!(TogglePseudocode, "Peek at pseudocode (Tab / F5)"),
                toggle(c!(ToggleSplit, "Pseudocode pane (toggle)"), view.split),
                c!(MoveToOtherPane, "Move tab to other pane"),
                c!(MoveToNewWindow, "Move tab to a new window"),
                toggle(c!(ToggleHideCasts, "Hide casts"), view.hide_casts),
                Entry::Sub {
                    label: "New tab".into(),
                    enabled: ready,
                    entries: vec![
                        c!(OpenInNewTab, "Open the current view in a new tab"),
                        Entry::Sep,
                        c!(NewListingTab, "Disassembly"),
                        c!(NewGraphTab, "Graph"),
                        c!(NewHexTab, "Hex"),
                        c!(NewPseudoTab, "Pseudocode"),
                    ],
                },
                Entry::Sep,
                Entry::Sub {
                    label: "Open subviews".into(),
                    enabled: ready,
                    entries: vec![
                        c!(ShowFunctions),
                        c!(ShowNames),
                        c!(ShowStrings),
                        c!(ShowImports),
                        c!(ShowExports),
                        c!(ShowSegments),
                        c!(ShowTypes),
                        Entry::Sep,
                        c!(ShowLocalTypes),
                        c!(ShowStructures),
                        c!(ShowEnums),
                        c!(ShowFrame),
                        c!(ShowSignatures),
                        c!(ShowPlugins),
                        c!(ShowProximity),
                        Entry::Sep,
                        c!(ShowPythonConsole),
                    ],
                },
                Entry::Sub {
                    label: "Toolbars and panels".into(),
                    enabled: ready,
                    entries: vec![
                        toggle(c!(ToggleSidebar, "Side panel"), view.sidebar),
                        toggle(c!(ToggleInspector, "Inspector"), view.inspector),
                        toggle(c!(ToggleBottomPanel, "Bottom panel"), view.bottom),
                        c!(ToggleOutput, "Output"),
                        c!(ShowXrefsPanel, "Live xrefs"),
                    ],
                },
                Entry::Sub {
                    label: "Graph".into(),
                    enabled: ready,
                    entries: vec![
                        c!(ZoomIn),
                        c!(ZoomOut),
                        c!(ZoomReset),
                        c!(ToggleMinimap, "Minimap"),
                    ],
                },
                Entry::Sep,
                c!(CommandPalette, "Command palette…"),
            ],
        ),
        m(
            "Debugger",
            0,
            vec![
                gate(
                    c!(DebugStart, "Start process / continue"),
                    ready && (!view.debug_active || view.debug_suspended),
                ),
                gate(c!(DebugPause), view.debug_active && !view.debug_suspended),
                gate(c!(DebugTerminate), view.debug_active),
                Entry::Sep,
                gate(c!(DebugAttach), ready && !view.debug_active),
                gate(c!(DebugDetach), view.debug_active),
                Entry::Sep,
                gate(c!(DebugStepInto), view.debug_suspended),
                gate(c!(DebugStepOver), view.debug_suspended),
                gate(c!(DebugStepOut), view.debug_suspended),
                gate(c!(DebugRunToCursor), view.debug_suspended),
                Entry::Sep,
                c!(ToggleBreakpoint),
                Entry::Sep,
                Entry::Sub {
                    label: "Debugger windows".into(),
                    enabled: ready,
                    entries: vec![
                        c!(ShowRegisters),
                        c!(ShowStack),
                        c!(ShowMemory),
                        c!(ShowCallStack),
                        c!(ShowThreads),
                        c!(ShowModules),
                        c!(ShowBreakpoints),
                        c!(ShowDebugEvents),
                        c!(ShowExceptions),
                    ],
                },
                Entry::Sep,
                gate(c!(HexProcessMemory), view.debug_suspended),
                Entry::Sep,
                c!(DebuggerOptions),
            ],
        ),
        m(
            "Options",
            0,
            vec![
                Entry::Sub {
                    label: "Theme".into(),
                    enabled: true,
                    entries: themes,
                },
                Entry::Sub {
                    label: "Code font size".into(),
                    enabled: true,
                    entries: sizes,
                },
                Entry::Sep,
                c!(OpenKeymap, "Keyboard shortcuts…"),
                c!(OpenSettings, "Settings…"),
            ],
        ),
        m(
            "Windows",
            0,
            vec![
                c!(NextDatabase),
                c!(PrevDatabase),
                Entry::Sep,
                c!(CloseTab),
                c!(ResetLayout),
            ]
            .into_iter()
            .chain((!dbs.is_empty()).then_some(Entry::Sep))
            .chain(dbs.iter().enumerate().map(|(ix, d)| {
                Entry::Item(Item {
                    label: d.name.clone(),
                    detail: SharedString::default(),
                    shortcut: if ix < 9 {
                        format!("Ctrl+{}", ix + 1)
                    } else {
                        Default::default()
                    }
                    .into(),
                    enabled: true,
                    checked: d.active,
                    target: Target::Database(d.id),
                })
            }))
            .collect(),
        ),
        m(
            "Help",
            0,
            vec![
                c!(ShowDocumentation, "Documentation"),
                c!(ReportIssue, "Report an issue…"),
                Entry::Sep,
                c!(ShowComingFromIda, "Coming from IDA…"),
                c!(ShowShortcuts, "Keyboard shortcuts"),
                Entry::Sep,
                c!(About, "About ida-zui…"),
            ],
        ),
    ]
}

pub struct MenuBar {
    /// Pushed by the workspace on every render: a database is open.
    pub ready: bool,
    /// The open databases, for the Windows menu.
    pub dbs: Vec<DbEntry>,
    /// The title that is highlighted; its dropdown shows when `dropped`.
    open: Option<usize>,
    dropped: bool,
    /// Highlighted entry per level; every level but the last is an open submenu.
    path: Vec<usize>,
    menus: Vec<Menu>,
    pub view: ViewState,
    long_scroll: [ScrollHandle; 3],
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    /// A context menu: the same dropdown machinery, opened at a window
    /// position instead of under a title.
    popup: Option<Popup>,
    /// Only context menus: no menu titles are drawn (a floating window).
    pub popup_only: bool,
    /// The caret's operand, for the check marks of Edit > Operand.
    pub ops: crate::operands::Ops,
    _sub: Subscription,
}

/// A context menu (see `MenuBar::open_popup`).
struct Popup {
    pos: Point<Pixels>,
    entries: Vec<Entry>,
    /// Submenus open to the left: the menu is near the right edge.
    sub_left: bool,
}

impl EventEmitter<MenuEvent> for MenuBar {}

impl MenuBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        // Focus moving elsewhere (a dialog opening, say) closes the menu.
        let sub = cx.on_focus_out(&focus, window, |this, _, _, cx| {
            if this.open.is_some() || this.popup.is_some() {
                this.open = None;
                this.popup = None;
                this.dropped = false;
                this.path.clear();
                this.return_focus = None;
                cx.notify();
            }
        });
        Self {
            ready: false,
            dbs: Vec::new(),
            open: None,
            dropped: false,
            path: Vec::new(),
            menus: Vec::new(),
            view: ViewState::default(),
            long_scroll: Default::default(),
            focus,
            return_focus: None,
            popup: None,
            popup_only: false,
            ops: Default::default(),
            _sub: sub,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some() || self.popup.is_some()
    }

    /// Opens a context menu at `pos` (window coordinates). The first entry is
    /// highlighted when `keyboard`; `title` heads the list.
    pub fn open_popup(
        &mut self,
        pos: Point<Pixels>,
        title: impl Into<SharedString>,
        mut entries: Vec<Entry>,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_open() {
            self.return_focus = window.focused(cx);
        }
        let title: SharedString = title.into();
        if !title.is_empty() {
            entries.insert(0, Entry::Header(title));
            entries.insert(1, Entry::Sep);
        }
        self.open = None;
        self.dropped = true;
        self.path.clear();
        let sub_left = f32::from(pos.x) > f32::from(window.viewport_size().width) - 560.;
        self.popup = Some(Popup {
            pos,
            entries,
            sub_left,
        });
        if keyboard {
            self.path.extend(self.step_from(0, None, 1));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn refresh(&mut self, cx: &App) {
        self.menus = build(self.ready, self.view, &self.dbs, &self.ops, cx);
    }

    /// Opens menu `ix`; `keyboard` highlights its first entry.
    pub fn open_menu(
        &mut self,
        ix: usize,
        dropped: bool,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_open() {
            self.return_focus = window.focused(cx);
        }
        self.popup = None;
        self.refresh(cx);
        self.open = Some(ix);
        self.dropped = dropped;
        self.path.clear();
        if dropped && keyboard {
            self.path.extend(self.step_from(0, None, 1));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Alt / F10: focus the bar on its first title.
    pub fn focus_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.close(window, cx);
        } else {
            self.open_menu(0, false, false, window, cx);
        }
    }

    pub fn open_by_index(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu(ix, true, true, window, cx);
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open() {
            return;
        }
        self.open = None;
        self.popup = None;
        self.dropped = false;
        self.path.clear();
        if let Some(f) = self.return_focus.take() {
            window.focus(&f, cx);
        }
        cx.notify();
    }

    fn entries_at(&self, level: usize) -> Option<&[Entry]> {
        let mut entries = match &self.popup {
            Some(p) => p.entries.as_slice(),
            None => self.menus.get(self.open?)?.entries.as_slice(),
        };
        for &ix in self.path.iter().take(level) {
            match entries.get(ix)? {
                Entry::Sub { entries: sub, .. } => entries = sub,
                _ => return None,
            }
        }
        Some(entries)
    }

    /// The next selectable entry after (`dir` 1) or before (`dir` -1) `from`
    /// at `level`, wrapping around.
    fn step_from(&self, level: usize, from: Option<usize>, dir: isize) -> Option<usize> {
        let entries = self.entries_at(level)?;
        let n = entries.len() as isize;
        let mut ix = match (from, dir) {
            (Some(f), _) => f as isize,
            (None, 1) => -1,
            (None, _) => n,
        };
        for _ in 0..n {
            ix = (ix + dir).rem_euclid(n);
            if entries[ix as usize].selectable() {
                return Some(ix as usize);
            }
        }
        None
    }

    fn highlight(&mut self, level: usize, ix: usize, cx: &mut Context<Self>) {
        if self.path.get(level) != Some(&ix) || self.path.len() != level + 1 {
            self.path.truncate(level);
            self.path.push(ix);
            cx.notify();
        }
    }

    fn select(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dropped {
            if dir > 0 {
                // Down on a bare title drops it.
                if let Some(ix) = self.open {
                    self.open_menu(ix, true, true, window, cx);
                }
            }
            return;
        }
        let level = self.path.len().saturating_sub(1);
        let from = self.path.last().copied();
        if let Some(ix) = self.step_from(level, from, dir) {
            self.path.truncate(level);
            self.path.push(ix);
            cx.notify();
        }
    }

    fn move_menu(&mut self, dir: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cur) = self.open else { return };
        let n = self.menus.len() as isize;
        let next = (cur as isize + dir).rem_euclid(n) as usize;
        self.open_menu(next, self.dropped, true, window, cx);
    }

    fn right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dropped
            && let Some(&ix) = self.path.last()
            && let Some(Entry::Sub {
                entries,
                enabled: true,
                ..
            }) = self.entries_at(self.path.len() - 1).and_then(|e| e.get(ix))
            && let Some(first) = entries.iter().position(Entry::selectable)
        {
            self.path.push(first);
            cx.notify();
        } else {
            self.move_menu(1, window, cx);
        }
    }

    fn left(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.len() > 1 {
            self.path.pop();
            cx.notify();
        } else {
            self.move_menu(-1, window, cx);
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dropped {
            if let Some(ix) = self.open {
                self.open_menu(ix, true, true, window, cx);
            }
            return;
        }
        let Some(&ix) = self.path.last() else { return };
        let level = self.path.len() - 1;
        match self.entries_at(level).and_then(|e| e.get(ix)).cloned() {
            Some(Entry::Item(item)) if item.enabled => self.invoke(item.target, window, cx),
            Some(Entry::Sub { .. }) => self.right(window, cx),
            _ => {}
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path.len() > 1 {
            self.path.pop();
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    fn invoke(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
        cx.emit(MenuEvent::Invoke(target));
    }

    /// A label with its mnemonic letter underlined.
    fn label(raw: &SharedString, mnemonic: Option<usize>) -> StyledText {
        let text: SharedString = parse_label(raw).0.into();
        let text = &text;
        match mnemonic {
            Some(at) => {
                let len = text[at..].chars().next().map_or(1, char::len_utf8);
                StyledText::new(text.clone()).with_highlights(vec![(
                    at..at + len,
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: None,
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                )])
            }
            None => StyledText::new(text.clone()),
        }
    }

    /// A letter pressed while a dropdown is open runs (or opens) the entry it underlines.
    fn on_letter(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.dropped || ev.keystroke.modifiers.control || ev.keystroke.modifiers.alt {
            return;
        }
        let Some(c) = ev
            .keystroke
            .key
            .chars()
            .next()
            .filter(|_| ev.keystroke.key.chars().count() == 1)
        else {
            return;
        };
        let level = self.path.len().saturating_sub(1);
        let Some(entries) = self.entries_at(level) else {
            return;
        };
        let marks = mnemonics(entries);
        let hit = entries.iter().enumerate().position(|(i, e)| {
            e.selectable()
                && marks[i].is_some_and(|at| {
                    let Some(label) = entry_label(e) else {
                        return false;
                    };
                    parse_label(label).0[at..]
                        .chars()
                        .next()
                        .is_some_and(|l| l.eq_ignore_ascii_case(&c))
                })
        });
        if let Some(ix) = hit {
            self.path.truncate(level);
            self.path.push(ix);
            self.confirm(window, cx);
        }
    }

    fn render_entries(
        &self,
        entries: &[Entry],
        level: usize,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let sub_left = self.popup.as_ref().is_some_and(|p| p.sub_left);
        let hi = self.path.get(level).copied();
        let marks = mnemonics(entries);
        let t = *theme;
        // A long menu (the plugin list) scrolls inside the window.
        let long = true;
        let handle = &self.long_scroll[level.min(2)];
        let max_h = f32::from(window.viewport_size().height) - BAR_HEIGHT - 60.;
        if long && let Some(h) = hi {
            handle.scroll_to_item(h);
        }
        let bar = long.then(|| {
            scrollbar::scrollbar(
                ("menu-scroll", level),
                Source::handle(handle),
                Axes::V,
                theme,
                window,
                cx,
            )
        });
        let list = div()
            .id(("menu-list", level))
            .flex()
            .flex_col()
            .text_size(px(12.))
            .line_height(px(16.))
            .when(long, |d| {
                d.max_h(px(max_h)).overflow_y_scroll().track_scroll(handle)
            })
            .children(entries.iter().enumerate().map(|(ix, e)| {
                match e {
                    Entry::Sep => div()
                        .h(px(1.))
                        .my(px(4.))
                        .bg(hsla(t.border))
                        .into_any_element(),
                    Entry::Header(text) => div()
                        .h(px(ROW_HEIGHT))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(hsla(t.muted))
                        .child(div().w(px(16.)).flex_none())
                        .child(
                            div()
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .child(text.clone()),
                        )
                        .into_any_element(),
                    Entry::Item(item) => {
                        let item = item.clone();
                        let target = item.target.clone();
                        let enabled = item.enabled;
                        let text = if enabled { hsla(t.text) } else { hsla(t.faint) };
                        div()
                            .id(("item", level * 1000 + ix))
                            .h(px(ROW_HEIGHT))
                            .px(px(8.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_color(text)
                            .when(enabled, |d| d.cursor_pointer())
                            .when(hi == Some(ix) && enabled, |d| d.bg(hsla(t.accent_soft)))
                            .on_hover(cx.listener(move |m, h: &bool, _, cx| {
                                if *h {
                                    m.highlight(level, ix, cx);
                                }
                            }))
                            .on_click(cx.listener(move |m, _, window, cx| {
                                if enabled {
                                    m.invoke(target.clone(), window, cx);
                                }
                            }))
                            .child(div().w(px(16.)).flex_none().when(item.checked, |d| {
                                d.child(icon("check", 16., hsla(t.accent)))
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .whitespace_nowrap()
                                    .child(Self::label(&item.label, marks[ix])),
                            )
                            .when(!item.detail.is_empty(), |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(hsla(t.faint))
                                        .child(item.detail.clone()),
                                )
                            })
                            .child(div().w(px(24.)).flex_none())
                            .child(
                                div()
                                    .font_family(code_font_name())
                                    .text_size(px(11.))
                                    .text_color(hsla(if enabled { t.muted } else { t.faint }))
                                    .child(item.shortcut.clone()),
                            )
                            .into_any_element()
                    }
                    Entry::Sub {
                        label,
                        enabled,
                        entries,
                    } => {
                        let enabled = *enabled;
                        let open = hi == Some(ix) && enabled;
                        let sub = open
                            .then(|| self.render_entries(entries, level + 1, theme, window, cx));
                        div()
                            .id(("sub", level * 1000 + ix))
                            .relative()
                            .h(px(ROW_HEIGHT))
                            .px(px(8.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_color(if enabled { hsla(t.text) } else { hsla(t.faint) })
                            .when(enabled, |d| d.cursor_pointer())
                            .when(open, |d| d.bg(hsla(t.accent_soft)))
                            .on_hover(cx.listener(move |m, h: &bool, _, cx| {
                                if *h && enabled {
                                    m.highlight(level, ix, cx);
                                }
                            }))
                            .child(div().w(px(16.)).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .whitespace_nowrap()
                                    .child(Self::label(label, marks[ix])),
                            )
                            .child(div().w(px(24.)).flex_none())
                            .child(icon("chevron-right", 12., hsla(t.muted)))
                            .when_some(sub, |d, sub| {
                                d.child(deferred(if sub_left {
                                    div()
                                        .absolute()
                                        .top(px(-5.))
                                        .right(relative(1.))
                                        .mr(px(-4.))
                                        .child(sub)
                                } else {
                                    div()
                                        .absolute()
                                        .top(px(-5.))
                                        .left(relative(1.))
                                        .pl(px(0.))
                                        .ml(px(-4.))
                                        .child(sub)
                                }))
                            })
                            .into_any_element()
                    }
                }
            }));
        div()
            .occlude()
            .relative()
            .min_w(px(240.))
            .p(px(4.))
            .rounded(px(8.))
            .bg(hsla(t.popover))
            .border_1()
            .border_color(hsla(t.border_strong))
            .shadow(ui::drop_shadow(&t, 8., 24.))
            .child(list)
            .children(bar)
    }
}

impl Focusable for MenuBar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        self.refresh(cx);
        let open = self.open;
        let dropped = self.dropped;
        let popup = self.popup.as_ref().map(|p| (p.pos, p.entries.clone()));
        let popup = popup.map(|(pos, entries)| {
            deferred(
                anchored()
                    .position(pos)
                    .child(self.render_entries(&entries, 0, &theme, window, cx)),
            )
        });
        let titles: Vec<_> = self
            .menus
            .iter()
            .enumerate()
            .map(|(ix, menu)| {
                let active = open == Some(ix);
                let text = StyledText::new(menu.title).with_highlights(vec![(
                    menu.mnemonic.clone(),
                    HighlightStyle {
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            color: None,
                            wavy: false,
                        }),
                        ..Default::default()
                    },
                )]);
                let dropdown = (active && dropped).then(|| {
                    let entries = self.menus[ix].entries.clone();
                    deferred(
                        div()
                            .absolute()
                            .top(px(BAR_HEIGHT - 2.))
                            .left(px(0.))
                            .child(self.render_entries(&entries, 0, &theme, window, cx)),
                    )
                });
                div()
                    .id(("menu", ix))
                    .relative()
                    .h(px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .rounded(px(4.))
                    .when(active, |d| d.bg(hsla(theme.accent_soft)))
                    .when(!active, |d| d.hover(|d| d.bg(hsla(theme.hover))))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |m, _, window, cx| {
                            if m.open == Some(ix) && m.dropped {
                                m.close(window, cx);
                            } else {
                                m.open_menu(ix, true, false, window, cx);
                            }
                        }),
                    )
                    .on_hover(cx.listener(move |m, h: &bool, window, cx| {
                        if *h && m.open.is_some() && m.open != Some(ix) {
                            m.open_menu(ix, m.dropped, false, window, cx);
                        }
                    }))
                    .child(text)
                    .when_some(dropdown, |d, dd| d.child(dd))
            })
            .collect();
        div()
            .id("menubar")
            .h_full()
            .flex()
            .items_center()
            .when(open.is_some() || popup.is_some(), |d| {
                d.key_context("MenuBar")
            })
            .track_focus(&self.focus)
            .on_action(cx.listener(|m, _: &SelectNext, window, cx| m.select(1, window, cx)))
            .on_action(cx.listener(|m, _: &SelectPrev, window, cx| m.select(-1, window, cx)))
            .on_action(cx.listener(|m, _: &MenuRight, window, cx| m.right(window, cx)))
            .on_action(cx.listener(|m, _: &MenuLeft, window, cx| m.left(window, cx)))
            .on_action(cx.listener(|m, _: &Confirm, window, cx| m.confirm(window, cx)))
            .on_action(cx.listener(|m, _: &Cancel, window, cx| m.cancel(window, cx)))
            .on_key_down(
                cx.listener(|m, ev: &KeyDownEvent, window, cx| m.on_letter(ev, window, cx)),
            )
            .when(!self.popup_only, |d| d.children(titles))
            .children(popup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> Entry {
        Entry::Item(Item {
            label: label.to_owned().into(),
            detail: SharedString::default(),
            shortcut: SharedString::default(),
            enabled: true,
            checked: false,
            target: Target::ClearRecent,
        })
    }

    #[test]
    fn mnemonics_take_the_first_free_letter_of_each_label() {
        let entries = [
            item("Open…"),
            item("Open with options…"),
            Entry::Sep,
            item("Save database"),
            item("Exit"),
        ];
        let marks = mnemonics(&entries);
        // "Open with options" cannot reuse O, so it takes p.
        assert_eq!(marks, vec![Some(0), Some(1), None, Some(0), Some(0)]);
    }

    #[test]
    fn an_ampersand_marks_the_mnemonic_and_is_not_shown() {
        assert_eq!(parse_label("Make &data"), ("Make data".to_owned(), Some(5)));
        assert_eq!(parse_label("No marker"), ("No marker".to_owned(), None));
        // `&&` is a literal ampersand, and a trailing `&` marks nothing.
        assert_eq!(
            parse_label("Save && &exit"),
            ("Save & exit".to_owned(), Some(7))
        );
        assert_eq!(parse_label("Tom &"), ("Tom ".to_owned(), None));
    }

    #[test]
    fn explicit_mnemonics_win_and_automatic_ones_stay_out_of_their_way() {
        // Marking the c and d keeps both Make entries natural (automatically the
        // second would get `a`), and a marked letter is never taken by an
        // unmarked label that comes before it.
        let entries = [
            item("Undo"),
            item("Make &code"),
            item("Make &data"),
            item("Undefine"),
            item("&Unmark"),
        ];
        let marks = mnemonics(&entries);
        // Undo gives U to &Unmark (n); Undefine finds U, n and d (Make &data) gone (e).
        assert_eq!(marks, vec![Some(1), Some(5), Some(5), Some(3), Some(0)]);
    }

    #[test]
    fn a_label_with_no_free_letter_has_no_mnemonic() {
        let entries = [item("a"), item("A")];
        assert_eq!(mnemonics(&entries), vec![Some(0), None]);
    }
}
