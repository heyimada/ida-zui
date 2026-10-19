// Every user command in one table: label, menu text, default keys, search
// aliases. The key bindings, the command palette, the menu bar, the shortcut
// editor and the help list are all derived from it, so they cannot disagree.
// Single-letter keys are bound only in the "CodeView" context so typing in an
// input never triggers them.
use gpui::{Action, App, KeyBinding, Keystroke, actions};
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

actions!(
    ida,
    [
        // navigation
        GoTo,
        Back,
        Forward,
        Follow,
        CursorUp,
        CursorDown,
        PageUp,
        PageDown,
        Top,
        Bottom,
        // views
        ToggleGraph,
        ShowGraph,
        TogglePseudocode,
        ShowHex,
        ShowListing,
        FocusSidebar,
        ToggleSidebar,
        ToggleOutput,
        ToggleBottomPanel,
        ToggleInspector,
        ToggleSplit,
        ShowPseudocode,
        ShowXrefsPanel,
        ShowPythonConsole,
        RunScriptFile,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        ToggleMinimap,
        ShowFunctions,
        ShowNames,
        ShowStrings,
        ShowImports,
        ShowExports,
        ShowSegments,
        ShowTypes,
        ShowLocalTypes,
        ShowStructures,
        ShowEnums,
        ShowFrame,
        ShowSignatures,
        ShowPlugins,
        ShowProximity,
        LuminaPullAll,
        LuminaPullFunction,
        LuminaPushAll,
        LuminaPushFunction,
        CloseTab,
        // annotation
        Rename,
        Comment,
        RepeatableComment,
        SetType,
        XrefsTo,
        XrefsFrom,
        // editing
        MakeCode,
        MakeData,
        MakeString,
        Undefine,
        AddField,
        DeleteMember,
        MakeFunction,
        DeleteFunction,
        ToggleCollapse,
        // search
        SearchBytes,
        SearchText,
        SearchNextBytes,
        SearchNextText,
        // database
        OpenFile,
        OpenWithOptions,
        LocateIda,
        CloseDatabase,
        NextDatabase,
        PrevDatabase,
        SwitchDatabase1,
        SwitchDatabase2,
        SwitchDatabase3,
        SwitchDatabase4,
        SwitchDatabase5,
        SwitchDatabase6,
        SwitchDatabase7,
        SwitchDatabase8,
        SwitchDatabase9,
        Save,
        Undo,
        Redo,
        CommandPalette,
        CopyAddress,
        Quit,
        // application
        OpenSettings,
        OpenKeymap,
        ShowShortcuts,
        About,
        // debugger
        DebugStart,
        DebugPause,
        DebugTerminate,
        DebugDetach,
        DebugAttach,
        DebugStepInto,
        DebugStepOver,
        DebugStepOut,
        DebugRunToCursor,
        ToggleBreakpoint,
        DebuggerOptions,
        ShowBreakpoints,
        ShowRegisters,
        ShowStack,
        ShowMemory,
        HexProcessMemory,
        ShowCallStack,
        ShowThreads,
        ShowModules,
        ShowDebugEvents,
        ShowExceptions,
        // list / modal / menu navigation
        ApplyDecl,
        Confirm,
        Cancel,
        SelectPrev,
        SelectNext,
        MenuLeft,
        MenuRight,
        FocusMenuBar,
        MenuFile,
        MenuEdit,
        MenuJump,
        MenuSearch,
        MenuView,
        MenuDebugger,
        MenuOptions,
        MenuWindows,
        MenuHelp,
    ]
);

const CV: &str = "CodeView";
const WS: &str = "Workspace";

pub struct Command {
    /// Namespaced action name, e.g. `ida::Rename`; the key of keymap overrides.
    pub name: &'static str,
    pub label: &'static str,
    pub make: Box<dyn Fn() -> Box<dyn Action> + Send + Sync>,
    /// Disabled in menus until a database is open.
    pub needs_db: bool,
    /// Listed in the command palette (navigation keys are not).
    pub palette: bool,
    /// Search aliases (IDA vocabulary and common synonyms).
    pub keywords: &'static str,
    /// Heading in the shortcut lists.
    pub group: &'static str,
    /// Default keystrokes and the key context each is bound in.
    pub keys: &'static [(&'static str, &'static str)],
}

macro_rules! cmd {
    ($a:ident, $group:expr, $label:expr, db: $db:expr, palette: $pal:expr, $kw:expr, $keys:expr) => {
        Command {
            name: <$a as Action>::name_for_type(),
            label: $label,
            make: Box::new(|| Box::new($a)),
            needs_db: $db,
            palette: $pal,
            keywords: $kw,
            group: $group,
            keys: $keys,
        }
    };
}

pub fn catalog() -> &'static [Command] {
    static CATALOG: OnceLock<Vec<Command>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        vec![
            cmd!(GoTo, "Navigation", "Jump to address or name", db: true, palette: true, "goto go navigate find symbol", &[("g", CV), ("ctrl-p", WS)]),
            cmd!(Back, "Navigation", "Back", db: true, palette: true, "history previous", &[("escape", CV), ("alt-left", WS)]),
            cmd!(Forward, "Navigation", "Forward", db: true, palette: true, "history next", &[("ctrl-enter", CV), ("alt-right", WS)]),
            cmd!(Follow, "Navigation", "Follow name under cursor", db: true, palette: false, "", &[("enter", CV)]),
            cmd!(CursorUp, "Navigation", "Cursor up", db: true, palette: false, "", &[("up", CV)]),
            cmd!(CursorDown, "Navigation", "Cursor down", db: true, palette: false, "", &[("down", CV)]),
            cmd!(PageUp, "Navigation", "Page up", db: true, palette: false, "", &[("pageup", CV)]),
            cmd!(PageDown, "Navigation", "Page down", db: true, palette: false, "", &[("pagedown", CV)]),
            cmd!(Top, "Navigation", "Go to start", db: true, palette: false, "", &[("ctrl-home", CV)]),
            cmd!(Bottom, "Navigation", "Go to end", db: true, palette: false, "", &[("ctrl-end", CV)]),
            cmd!(XrefsTo, "Navigation", "Cross references to", db: true, palette: true, "xrefs xref references callers usages", &[("x", CV), ("ctrl-x", CV)]),
            cmd!(XrefsFrom, "Navigation", "Cross references from", db: true, palette: true, "xrefs xref references callees", &[("ctrl-j", CV), ("shift-x", CV)]),
            cmd!(ToggleGraph, "Views", "Toggle graph / linear view", db: true, palette: true, "cfg flow chart blocks", &[("space", CV)]),
            cmd!(ShowGraph, "Views", "Show graph view", db: true, palette: true, "cfg flow chart blocks", &[]),
            cmd!(TogglePseudocode, "Views", "Toggle pseudocode", db: true, palette: true, "decompile hex-rays f5 c code", &[("tab", CV), ("f5", WS)]),
            cmd!(ShowHex, "Views", "Show hex view", db: true, palette: true, "bytes dump memory", &[("ctrl-shift-h", WS)]),
            cmd!(ShowListing, "Views", "Show disassembly", db: true, palette: true, "listing linear assembly", &[("ctrl-shift-d", WS)]),
            cmd!(FocusSidebar, "Views", "Focus side panel", db: true, palette: true, "filter lists", &[("ctrl-shift-b", WS)]),
            cmd!(ToggleSidebar, "Views", "Show / hide side panel", db: true, palette: true, "lists", &[]),
            cmd!(ToggleOutput, "Views", "Toggle output panel", db: true, palette: true, "log messages console", &[("ctrl-`", WS)]),
            cmd!(ToggleBottomPanel, "Views", "Show / hide bottom panel", db: true, palette: true, "output xrefs panel", &[("ctrl-alt-j", WS)]),
            cmd!(ShowXrefsPanel, "Views", "Show live xrefs panel", db: true, palette: true, "references callers bottom", &[]),
            cmd!(ShowPythonConsole, "Views", "Show Python console", db: true, palette: true, "idapython script repl shell eval", &[]),
            cmd!(RunScriptFile, "File", "Run script file", db: true, palette: true, "python idapython idc execute alt f7", &[("alt-f7", WS)]),
            cmd!(ToggleInspector, "Views", "Show / hide inspector", db: true, palette: true, "info properties right panel", &[("ctrl-alt-i", WS)]),
            cmd!(ToggleSplit, "Views", "Toggle split view with pseudocode", db: true, palette: true, "side by side decompile", &[]),
            cmd!(ShowPseudocode, "Views", "Show pseudocode", db: true, palette: true, "decompile hex-rays c code", &[]),
            cmd!(ZoomIn, "Views", "Zoom in", db: true, palette: true, "graph larger", &[("ctrl-=", CV)]),
            cmd!(ZoomOut, "Views", "Zoom out", db: true, palette: true, "graph smaller", &[("ctrl--", CV)]),
            cmd!(ZoomReset, "Views", "Reset zoom", db: true, palette: true, "graph", &[("ctrl-0", CV)]),
            cmd!(ToggleMinimap, "Views", "Show / hide graph minimap", db: true, palette: true, "graph overview map", &[]),
            cmd!(ShowFunctions, "Windows", "Functions", db: true, palette: true, "side panel list", &[("shift-f3", WS)]),
            cmd!(ShowNames, "Windows", "Names", db: true, palette: true, "side panel list symbols", &[("shift-f4", WS)]),
            cmd!(ShowStrings, "Windows", "Strings", db: true, palette: true, "side panel list", &[("shift-f12", WS)]),
            cmd!(ShowImports, "Windows", "Imports", db: true, palette: true, "side panel list", &[]),
            cmd!(ShowExports, "Windows", "Exports", db: true, palette: true, "side panel list entry points", &[]),
            cmd!(ShowSegments, "Windows", "Segments", db: true, palette: true, "side panel list sections", &[("shift-f7", WS)]),
            cmd!(ShowTypes, "Windows", "Types", db: true, palette: true, "side panel list structures local types", &[]),
            cmd!(ShowLocalTypes, "Windows", "Local types", db: true, palette: true, "structures unions enums typedef declarations editor", &[("shift-f1", WS)]),
            cmd!(ShowStructures, "Windows", "Structures…", db: true, palette: true, "struct union members fields editor", &[("shift-f9", WS)]),
            cmd!(ShowEnums, "Windows", "Enums…", db: true, palette: true, "enumeration constants flags editor", &[("shift-f10", WS)]),
            cmd!(ShowFrame, "Windows", "Stack frame", db: true, palette: true, "locals variables arguments function frame editor", &[("ctrl-k", WS)]),
            cmd!(ShowSignatures, "Windows", "Signatures", db: true, palette: true, "flirt sig library functions apply list", &[("shift-f5", WS)]),
            cmd!(ShowPlugins, "Windows", "Plugins", db: true, palette: true, "plugin extension run hotkey", &[]),
            cmd!(ShowProximity, "Windows", "Proximity browser", db: true, palette: true, "call graph callers callees neighbourhood xref tree", &[]),
            cmd!(LuminaPullAll, "Lumina", "Lumina: pull metadata for all functions", db: true, palette: true, "lumina pull download metadata server names", &[]),
            cmd!(LuminaPullFunction, "Lumina", "Lumina: pull metadata for the current function", db: true, palette: true, "lumina pull download metadata server", &[]),
            cmd!(LuminaPushAll, "Lumina", "Lumina: push metadata for all functions…", db: true, palette: true, "lumina push upload metadata server share", &[]),
            cmd!(LuminaPushFunction, "Lumina", "Lumina: push metadata for the current function…", db: true, palette: true, "lumina push upload metadata server share", &[]),
            cmd!(DebugStart, "Debugger", "Start process / continue", db: true, palette: true, "debug run go resume f9 launch", &[("f9", WS)]),
            cmd!(DebugPause, "Debugger", "Pause process", db: true, palette: true, "debug suspend break halt", &[("f12", WS)]),
            cmd!(DebugTerminate, "Debugger", "Terminate process", db: true, palette: true, "debug stop kill exit end", &[("ctrl-f2", WS)]),
            cmd!(DebugDetach, "Debugger", "Detach from process", db: true, palette: true, "debug release", &[]),
            cmd!(DebugAttach, "Debugger", "Attach to process…", db: true, palette: true, "debug pid running", &[]),
            cmd!(DebugStepInto, "Debugger", "Step into", db: true, palette: true, "debug trace single instruction f7", &[("f7", WS)]),
            cmd!(DebugStepOver, "Debugger", "Step over", db: true, palette: true, "debug next f8", &[("f8", WS)]),
            cmd!(DebugStepOut, "Debugger", "Run until return", db: true, palette: true, "debug step out finish leave function", &[("ctrl-f7", WS)]),
            cmd!(DebugRunToCursor, "Debugger", "Run to cursor", db: true, palette: true, "debug here go to f4", &[("f4", WS)]),
            cmd!(ToggleBreakpoint, "Debugger", "Toggle breakpoint", db: true, palette: true, "debug bpt break set remove f2", &[("f2", WS)]),
            cmd!(DebuggerOptions, "Debugger", "Debugger options…", db: false, palette: true, "debug settings application arguments directory exceptions", &[]),
            cmd!(ShowBreakpoints, "Debugger", "Breakpoint list", db: true, palette: true, "debug bpt breakpoints panel", &[("ctrl-alt-b", WS)]),
            cmd!(ShowRegisters, "Debugger", "Registers", db: true, palette: true, "debug cpu flags panel", &[]),
            cmd!(ShowStack, "Debugger", "Stack", db: true, palette: true, "debug rsp esp panel", &[]),
            cmd!(HexProcessMemory, "Debugger", "Hex view: process memory (toggle)", db: true, palette: true, "refresh memory process snapshot dump live", &[]),
            cmd!(ShowMemory, "Debugger", "Memory", db: true, palette: true, "debug dump hex process panel", &[]),
            cmd!(ShowCallStack, "Debugger", "Call stack", db: true, palette: true, "debug backtrace frames panel", &[]),
            cmd!(ShowThreads, "Debugger", "Threads", db: true, palette: true, "debug panel", &[]),
            cmd!(ShowModules, "Debugger", "Modules", db: true, palette: true, "debug dll libraries panel", &[]),
            cmd!(ShowDebugEvents, "Debugger", "Debugger events", db: true, palette: true, "debug log output panel", &[]),
            cmd!(ShowExceptions, "Debugger", "Exceptions", db: true, palette: true, "debug pass stop handling panel", &[]),
            cmd!(CloseTab, "Views", "Close editor tab", db: true, palette: true, "close document", &[("ctrl-f4", WS)]),
            cmd!(AddField, "Editing", "Add field / member", db: true, palette: true, "structure enum frame variable insert", &[("insert", CV)]),
            cmd!(DeleteMember, "Editing", "Delete member", db: true, palette: true, "structure enum frame variable remove", &[("delete", CV)]),
            cmd!(Rename, "Editing", "Rename", db: true, palette: true, "name label symbol", &[("n", CV)]),
            cmd!(Comment, "Editing", "Comment", db: true, palette: true, "note", &[("shift-;", CV)]),
            cmd!(RepeatableComment, "Editing", "Repeatable comment", db: true, palette: true, "note", &[(";", CV)]),
            cmd!(SetType, "Editing", "Set type", db: true, palette: true, "prototype signature declaration", &[("y", CV)]),
            cmd!(MakeCode, "Editing", "Make code", db: true, palette: true, "disassemble convert", &[("c", CV)]),
            cmd!(MakeData, "Editing", "Make data (cycle size)", db: true, palette: true, "byte word dword qword define", &[("d", CV)]),
            cmd!(MakeString, "Editing", "Make string", db: true, palette: true, "ascii", &[("a", CV)]),
            cmd!(Undefine, "Editing", "Undefine", db: true, palette: true, "delete items unexplored", &[("u", CV)]),
            cmd!(MakeFunction, "Editing", "Create function", db: true, palette: true, "define proc", &[("p", CV)]),
            cmd!(DeleteFunction, "Editing", "Delete function", db: true, palette: true, "remove", &[("shift-delete", CV)]),
            cmd!(ToggleCollapse, "Editing", "Expand / collapse", db: true, palette: true, "unhide hide fold", &[("-", CV)]),
            cmd!(SearchBytes, "Search", "Search bytes", db: true, palette: true, "binary pattern find", &[("alt-b", WS)]),
            cmd!(SearchText, "Search", "Search text", db: true, palette: true, "string find", &[("alt-t", WS)]),
            cmd!(SearchNextBytes, "Search", "Search bytes again", db: true, palette: true, "binary pattern find next repeat", &[("ctrl-b", WS)]),
            cmd!(SearchNextText, "Search", "Search text again", db: true, palette: true, "string find next repeat", &[("ctrl-t", WS)]),
            cmd!(OpenFile, "File", "Open file", db: false, palette: true, "load binary idb i64", &[("ctrl-o", WS)]),
            cmd!(OpenWithOptions, "File", "Open file with options", db: false, palette: true, "loader processor raw binary base address", &[("ctrl-shift-o", WS)]),
            cmd!(LocateIda, "File", "Locate IDA installation", db: false, palette: true, "ida folder idalib path setup", &[]),
            cmd!(CloseDatabase, "File", "Close database", db: true, palette: true, "unload", &[("ctrl-w", WS)]),
            cmd!(NextDatabase, "Databases", "Next database", db: false, palette: true, "tab window switch cycle", &[("ctrl-tab", WS)]),
            cmd!(PrevDatabase, "Databases", "Previous database", db: false, palette: true, "tab window switch cycle", &[("ctrl-shift-tab", WS)]),
            cmd!(SwitchDatabase1, "Databases", "Switch to database 1", db: false, palette: false, "tab window", &[("ctrl-1", WS)]),
            cmd!(SwitchDatabase2, "Databases", "Switch to database 2", db: false, palette: false, "tab window", &[("ctrl-2", WS)]),
            cmd!(SwitchDatabase3, "Databases", "Switch to database 3", db: false, palette: false, "tab window", &[("ctrl-3", WS)]),
            cmd!(SwitchDatabase4, "Databases", "Switch to database 4", db: false, palette: false, "tab window", &[("ctrl-4", WS)]),
            cmd!(SwitchDatabase5, "Databases", "Switch to database 5", db: false, palette: false, "tab window", &[("ctrl-5", WS)]),
            cmd!(SwitchDatabase6, "Databases", "Switch to database 6", db: false, palette: false, "tab window", &[("ctrl-6", WS)]),
            cmd!(SwitchDatabase7, "Databases", "Switch to database 7", db: false, palette: false, "tab window", &[("ctrl-7", WS)]),
            cmd!(SwitchDatabase8, "Databases", "Switch to database 8", db: false, palette: false, "tab window", &[("ctrl-8", WS)]),
            cmd!(SwitchDatabase9, "Databases", "Switch to database 9", db: false, palette: false, "tab window", &[("ctrl-9", WS)]),
            cmd!(Save, "File", "Save database", db: true, palette: true, "write idb", &[("ctrl-s", WS)]),
            cmd!(Undo, "Edit", "Undo", db: true, palette: true, "revert", &[("ctrl-z", WS)]),
            cmd!(Redo, "Edit", "Redo", db: true, palette: true, "repeat", &[("ctrl-shift-z", WS), ("ctrl-y", WS)]),
            cmd!(CopyAddress, "Edit", "Copy current address", db: true, palette: true, "clipboard ea", &[("ctrl-shift-c", WS)]),
            cmd!(CommandPalette, "Application", "Command palette", db: false, palette: false, "", &[("ctrl-shift-p", WS)]),
            cmd!(OpenSettings, "Application", "Settings", db: false, palette: true, "preferences options theme font keymap", &[("ctrl-,", WS)]),
            cmd!(OpenKeymap, "Application", "Keyboard shortcut editor", db: false, palette: true, "keymap rebind hotkeys", &[]),
            cmd!(ShowShortcuts, "Application", "Keyboard shortcuts", db: false, palette: true, "keys hotkeys help", &[("f1", WS)]),
            cmd!(About, "Application", "About ida-zui", db: false, palette: true, "version info", &[]),
            cmd!(FocusMenuBar, "Application", "Focus menu bar", db: false, palette: false, "", &[("f10", WS)]),
            cmd!(Quit, "File", "Quit", db: false, palette: true, "exit close", &[("ctrl-q", WS)]),
        ]
    })
}

static PLUGIN_COMMANDS: RwLock<Vec<&'static Command>> = RwLock::new(Vec::new());

/// The commands made from the database's plugins (see `plugins.rs`).
pub fn set_plugin_commands(list: Vec<&'static Command>) {
    if let Ok(mut l) = PLUGIN_COMMANDS.write() {
        *l = list;
    }
}

/// The catalog plus the plugin commands: everything with a name, a label and keys.
pub fn commands() -> Vec<&'static Command> {
    let plugins = PLUGIN_COMMANDS
        .read()
        .map(|l| l.clone())
        .unwrap_or_default();
    catalog().iter().chain(plugins).collect()
}

pub fn find(name: &str) -> Option<&'static Command> {
    commands().into_iter().find(|c| c.name == name)
}

/// Bindings that cannot be changed: modal, list and menu navigation, and the
/// Alt+letter menu mnemonics.
fn fixed() -> Vec<(&'static str, Box<dyn Action>, &'static str)> {
    vec![
        ("enter", Box::new(Confirm), "Modal"),
        ("escape", Box::new(Cancel), "Modal"),
        ("up", Box::new(SelectPrev), "Modal"),
        ("down", Box::new(SelectNext), "Modal"),
        ("enter", Box::new(Confirm), "List"),
        ("enter", Box::new(Confirm), "CellEdit"),
        ("escape", Box::new(Cancel), "CellEdit"),
        ("ctrl-s", Box::new(ApplyDecl), "TypeEditor"),
        ("up", Box::new(SelectPrev), "List"),
        ("down", Box::new(SelectNext), "List"),
        ("enter", Box::new(Confirm), "MenuBar"),
        ("space", Box::new(Confirm), "MenuBar"),
        ("escape", Box::new(Cancel), "MenuBar"),
        ("up", Box::new(SelectPrev), "MenuBar"),
        ("down", Box::new(SelectNext), "MenuBar"),
        ("left", Box::new(MenuLeft), "MenuBar"),
        ("right", Box::new(MenuRight), "MenuBar"),
        ("alt-f", Box::new(MenuFile), WS),
        ("alt-e", Box::new(MenuEdit), WS),
        ("alt-j", Box::new(MenuJump), WS),
        ("alt-s", Box::new(MenuSearch), WS),
        ("alt-v", Box::new(MenuView), WS),
        ("alt-d", Box::new(MenuDebugger), WS),
        ("alt-o", Box::new(MenuOptions), WS),
        ("alt-w", Box::new(MenuWindows), WS),
        ("alt-h", Box::new(MenuHelp), WS),
        ("alt", Box::new(FocusMenuBar), WS),
    ]
}

/// Context for a user-chosen key: plain typing keys only work in the code
/// views so they never fire while typing into an input.
fn context_for(keystroke: &Keystroke) -> &'static str {
    let m = &keystroke.modifiers;
    let plain =
        !m.control && !m.alt && !m.platform && !m.function && keystroke.key.chars().count() == 1;
    if plain { CV } else { WS }
}

fn binding(keys: &str, action: Box<dyn Action>, context: &str) -> Option<KeyBinding> {
    let predicate = gpui::KeyBindingContextPredicate::parse(context).ok()?;
    match KeyBinding::load(
        keys,
        action,
        Some(predicate.into()),
        false,
        None,
        &gpui::DummyKeyboardMapper,
    ) {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("keymap: ignoring '{keys}': {e}");
            None
        }
    }
}

/// Rebinds every key: defaults, with `user` replacing a command's keys.
/// Replaces whatever was bound before, including the input and fixed bindings.
pub fn apply_keymap(user: &BTreeMap<String, Vec<String>>, cx: &mut App) {
    cx.clear_key_bindings();
    let mut all = Vec::new();
    for c in commands() {
        match user.get(c.name) {
            Some(keys) => {
                for k in keys {
                    let Ok(ks) = Keystroke::parse(k) else {
                        eprintln!("keymap: ignoring invalid key '{k}' for {}", c.name);
                        continue;
                    };
                    all.extend(binding(k, (c.make)(), context_for(&ks)));
                }
            }
            None => {
                for (k, ctx) in c.keys {
                    all.extend(binding(k, (c.make)(), ctx));
                }
            }
        }
    }
    for (k, action, ctx) in fixed() {
        all.extend(binding(k, action, ctx));
    }
    cx.bind_keys(all);
    crate::input::bind_keys(cx);
    zui_editor::bind_keys(cx);
}

/// A keystroke as people write it: `Ctrl+Shift+P`, `Alt+←`, `F5`.
pub fn pretty(k: &Keystroke) -> String {
    let mut s = String::new();
    let m = &k.modifiers;
    if m.control {
        s.push_str("Ctrl+");
    }
    if m.alt {
        s.push_str("Alt+");
    }
    if m.shift {
        s.push_str("Shift+");
    }
    if m.platform {
        s.push_str("Win+");
    }
    s.push_str(&match k.key.as_str() {
        "enter" => "Enter".to_owned(),
        "escape" => "Esc".to_owned(),
        "delete" => "Del".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "space" => "Space".to_owned(),
        "tab" => "Tab".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "pageup" => "PgUp".to_owned(),
        "pagedown" => "PgDn".to_owned(),
        "alt" => "Alt".to_owned(),
        key if key.chars().count() > 1 => {
            let mut c = key.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        }
        key => key.to_uppercase(),
    });
    s
}

pub fn pretty_str(keys: &str) -> String {
    keys.split_whitespace()
        .map(|part| Keystroke::parse(part).map_or_else(|_| part.to_owned(), |k| pretty(&k)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The keystrokes now bound to `name`, read from the live keymap (so keymap
/// changes show up) without regard to the focused context.
pub fn bound_keys(action: &dyn Action, cx: &App) -> Vec<String> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let mut out: Vec<String> = Vec::new();
    for b in keymap.bindings_for_action(action) {
        let text = b
            .keystrokes()
            .iter()
            .map(|k| pretty(k.inner()))
            .collect::<Vec<_>>()
            .join(" ");
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// Shortcut text for menus and the palette: up to two alternatives.
pub fn shortcut_text(action: &dyn Action, cx: &App) -> String {
    bound_keys(action, cx)
        .into_iter()
        .take(2)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Another command already using `keys` (compared as canonical keystrokes).
pub fn conflict(
    name: &str,
    keys: &str,
    user: &BTreeMap<String, Vec<String>>,
) -> Option<&'static Command> {
    let want = Keystroke::parse(keys).ok()?.unparse();
    commands().into_iter().find(|c| {
        c.name != name
            && effective_keys(c, user)
                .iter()
                .any(|k| Keystroke::parse(k).is_ok_and(|k| k.unparse() == want))
    })
}

/// The keystrokes a command has under `user` overrides.
pub fn effective_keys(c: &Command, user: &BTreeMap<String, Vec<String>>) -> Vec<String> {
    match user.get(c.name) {
        Some(keys) => keys.clone(),
        None => c.keys.iter().map(|(k, _)| (*k).to_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_key_parses() {
        // A typo here would be skipped at startup and leave a command unbound.
        for c in catalog() {
            for (k, _) in c.keys {
                assert!(Keystroke::parse(k).is_ok(), "{}: bad key '{k}'", c.name);
            }
        }
    }

    #[test]
    fn command_names_are_unique() {
        let mut names: Vec<_> = catalog().iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), catalog().len());
    }

    #[test]
    fn default_keys_do_not_collide() {
        let none = BTreeMap::new();
        for c in catalog() {
            for k in effective_keys(c, &none) {
                assert!(
                    conflict(c.name, &k, &none).is_none(),
                    "{} shares '{k}'",
                    c.name
                );
            }
        }
    }

    #[test]
    fn conflicts_see_overrides_and_spelling_variants() {
        let mut user = BTreeMap::new();
        user.insert("ida::Rename".to_owned(), vec!["ctrl-alt-r".to_owned()]);
        // Ctrl+Alt+R is now Rename's, however it is written.
        assert_eq!(
            conflict("ida::Save", "ctrl-alt-r", &user).map(|c| c.name),
            Some("ida::Rename")
        );
        // Rename's old key is free.
        assert!(conflict("ida::Save", "n", &user).is_none());
    }

    #[test]
    fn typed_keys_stay_out_of_inputs() {
        assert_eq!(context_for(&Keystroke::parse("n").unwrap()), "CodeView");
        assert_eq!(
            context_for(&Keystroke::parse("ctrl-j").unwrap()),
            "Workspace"
        );
        assert_eq!(context_for(&Keystroke::parse("f5").unwrap()), "Workspace");
    }

    #[test]
    fn keys_read_like_menus_write_them() {
        assert_eq!(pretty_str("ctrl-shift-p"), "Ctrl+Shift+P");
        assert_eq!(pretty_str("alt-left"), "Alt+←");
        assert_eq!(pretty_str("ctrl-home"), "Ctrl+Home");
    }
}
