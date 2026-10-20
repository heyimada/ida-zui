// Help dialogs: About (versions and paths, IDA's from the host) and the
// searchable keyboard shortcut list, read from the live keymap.
use crate::actions::{self, Cancel, Confirm};
use crate::dialog::{ButtonKind, DialogEvent, body, button, caption, footer, frame, header};
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::session::Session;
use crate::settings;
use crate::theme::{Theme, hsla};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use ida_proto::client::HostConfig;
use ida_proto::{Client, Version, VersionInfo};

/// The versions and paths shown by About: IDA's from the host. Asks the open
/// database's host, or starts a throwaway one when nothing is open.
pub fn fetch_version(
    session: &Entity<Session>,
    cx: &App,
) -> impl std::future::Future<Output = Result<VersionInfo, SharedString>> + 'static + use<> {
    let running = session.read(cx).call(Version {});
    let transient = if session.read(cx).client_is_none() {
        Client::spawn(&HostConfig::discover())
            .map(|(c, _logs)| c)
            .map_err(|e| e.to_string())
            .ok()
    } else {
        None
    };
    let fut = transient.as_ref().map(|c| c.call(Version {}));
    async move {
        let result = match fut {
            Some(f) => f.await,
            None => running.await,
        };
        if let Some(c) = transient {
            c.kill();
        }
        result.map_err(|e| e.to_string().into())
    }
}

/// The About rows: ida-zui's own, then IDA's once the host has answered.
pub fn about_rows(
    theme: &Theme,
    info: Option<&Result<VersionInfo, SharedString>>,
) -> Vec<gpui::Div> {
    let theme = *theme;
    let row = move |label: &'static str, value: String| {
        div()
            .flex()
            .gap(px(12.))
            .child(
                div()
                    .w(px(110.))
                    .flex_none()
                    .text_color(hsla(theme.muted))
                    .child(label),
            )
            .child(div().flex_1().min_w_0().child(value))
    };
    let mut rows = vec![
        row("ida-zui", env!("CARGO_PKG_VERSION").to_owned()),
        row("Settings", settings::settings_path().display().to_string()),
    ];
    match info {
        None => rows.push(row("IDA", "asking the IDA host…".to_owned())),
        Some(Ok(v)) => {
            rows.push(row("IDA", v.ida_version.clone()));
            rows.push(row("IDADIR", v.ida_dir.clone()));
            rows.push(row("IDA user dir", v.user_dir.clone()));
            rows.push(row("SDK", v.sdk_dir.clone()));
            rows.push(row("Host", v.host_exe.clone()));
        }
        Some(Err(e)) => rows.push(row("IDA", format!("unavailable: {e}"))),
    }
    rows
}

pub const ABOUT_BLURB: &str =
    "A modern front end for IDA Pro, built on zui. The IDA kernel runs headless through idalib.";

pub struct AboutDialog {
    info: Option<Result<VersionInfo, SharedString>>,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent> for AboutDialog {}

impl AboutDialog {
    /// Asks the open database's host for its versions, or starts a throwaway
    /// host when nothing is open (the welcome screen has none).
    pub fn new(session: &Entity<Session>, cx: &mut Context<Self>) -> Self {
        let fut = fetch_version(session, cx);
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |d, cx| {
                d.info = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            info: None,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for AboutDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AboutDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let rows = about_rows(&theme, self.info.as_ref());
        frame(&theme, 640.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(DialogEvent::Close)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &theme,
                "ida-zui",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(caption(&theme, ABOUT_BLURB))
                    .child(div().flex().flex_col().gap(px(6.)).children(rows)),
            )
            .child(
                footer(&theme).child(
                    button("close", "Close", ButtonKind::Primary, &theme)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                ),
            )
    }
}

pub const DOCS_URL: &str = "https://github.com/heyimada/ida-zui#readme";
pub const ISSUES_URL: &str = "https://github.com/heyimada/ida-zui/issues/new";

/// "Coming from IDA": the keys that are the same, and where ida-zui differs.
/// The README carries the same table.
pub const COMING_FROM_IDA: &[(&str, &str, &str)] = &[
    (
        "Jump to address / name",
        "G",
        "G (fuzzy over functions and names)",
    ),
    (
        "Jump to function / segment / entry point",
        "Ctrl+P / Ctrl+S / Ctrl+E",
        "Ctrl+P / Ctrl+Alt+S / Ctrl+E (Ctrl+S saves here)",
    ),
    ("Next / previous function", "Ctrl+Down / Ctrl+Up", "same"),
    (
        "Back / forward",
        "Esc / Ctrl+Enter",
        "same (plus Alt+Left / Alt+Right)",
    ),
    ("Follow", "Enter", "Enter, double-click"),
    (
        "Xrefs to / from",
        "X, Ctrl+X / Ctrl+J",
        "same (plus Shift+X for from)",
    ),
    ("Rename", "N", "N"),
    (
        "Comment / repeatable comment",
        ": / ;",
        "same (/ in pseudocode too)",
    ),
    ("Graph / linear", "Space", "Space"),
    ("Pseudocode", "F5, Tab", "F5, Tab (Tab again returns)"),
    (
        "Search text / bytes, again",
        "Alt+T / Alt+B, Ctrl+T / Ctrl+B",
        "same",
    ),
    ("Search immediate value, again", "Alt+I, Ctrl+I", "same"),
    (
        "Next code / data / unexplored",
        "Alt+C / Ctrl+D / Ctrl+U",
        "same",
    ),
    ("Next code outside a function", "Alt+U", "Alt+U"),
    (
        "Functions / Names / Strings / Segments",
        "Shift+F3 / Shift+F4 / Shift+F12 / Shift+F7",
        "same (side panel tabs)",
    ),
    ("Local types / Signatures", "Shift+F1 / Shift+F5", "same"),
    (
        "Code / data / string / undefine / function",
        "C D A U P",
        "same",
    ),
    (
        "Operand: hex/decimal, char, binary, offset, struct offset, enum, stack variable",
        "H R B O T M K",
        "same (negate _, bitwise not ~; Edit > Operand types)",
    ),
    (
        "Array / string type / edit function",
        "* / Alt+A / Alt+P",
        "same",
    ),
    (
        "Select lines, copy",
        "Shift+arrows, Ctrl+C",
        "Shift+arrows, Shift+click, drag, Ctrl+C (Esc clears)",
    ),
    (
        "Context menu",
        "right click",
        "right click, Menu key or Ctrl+Alt+M",
    ),
    (
        "Save database",
        "Ctrl+W",
        "Ctrl+W (Ctrl+S too); close database is Ctrl+Shift+W",
    ),
    ("Undo / redo", "Ctrl+Z / Ctrl+Shift+Z", "same (Ctrl+Y too)"),
    ("Debugger", "F9 F7 F8 F2 Ctrl+F2 F4", "same"),
];

pub const COMING_FROM_IDA_NOTES: &str = "The side lists live in a rail on the left instead of floating windows. \
Because Ctrl+B is IDA's \"search bytes again\", the side-panel filter moved to Ctrl+Shift+B. \
A database that was just analysed counts as unsaved. There is no database snapshot manager, \
plugins that need IDA's Qt UI report that they cannot run headless, and the debugger is Windows user mode only.";

/// The same-key-different-place table for people who know IDA.
pub struct ComingFromIdaDialog {
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent> for ComingFromIdaDialog {}

impl ComingFromIdaDialog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for ComingFromIdaDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ComingFromIdaDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let sh = scrollbar::handle("coming-h", window, cx);
        let sbar = scrollbar::scrollbar(
            "coming-scroll",
            Source::handle(&sh),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let head = |text: &'static str, w: f32| {
            div()
                .w(px(w))
                .flex_none()
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(hsla(theme.muted))
                .child(text.to_uppercase())
        };
        frame(&theme, 760.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &theme,
                "Coming from IDA",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(caption(
                        &theme,
                        "The defaults follow IDA's keys where IDA has one. Every key is editable in Options > Keyboard shortcuts.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .child(head("Action", 250.))
                            .child(head("IDA", 190.))
                            .child(head("ida-zui", 240.)),
                    )
                    .child(
                        div()
                            .relative()
                            .h(px(340.))
                            .child(
                                div()
                                    .id("coming-list")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&sh)
                                    .flex()
                                    .flex_col()
                                    .children(COMING_FROM_IDA.iter().map(|(action, ida, here)| {
                                        div()
                                            .flex()
                                            .gap(px(12.))
                                            .py(px(3.))
                                            .child(div().w(px(250.)).flex_none().child(*action))
                                            .child(
                                                div()
                                                    .w(px(190.))
                                                    .flex_none()
                                                    .text_color(hsla(theme.muted))
                                                    .child(*ida),
                                            )
                                            .child(div().w(px(240.)).flex_none().child(*here))
                                    })),
                            )
                            .child(sbar),
                    )
                    .child(caption(&theme, COMING_FROM_IDA_NOTES)),
            )
            .child(
                footer(&theme).child(
                    button("close", "Close", ButtonKind::Primary, &theme)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                ),
            )
    }
}

/// Every command with its current keys, filterable.
pub struct ShortcutsDialog {
    filter: Entity<TextInput>,
    focus: FocusHandle,
    _sub: Subscription,
}

impl EventEmitter<DialogEvent> for ShortcutsDialog {}

impl ShortcutsDialog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextInput::new("", "Filter commands or keys", cx));
        let sub = cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify());
        Self {
            filter,
            focus: cx.focus_handle(),
            _sub: sub,
        }
    }
}

impl Focusable for ShortcutsDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.read(cx).focus_handle(cx)
    }
}

impl Render for ShortcutsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let sh = scrollbar::handle("shortcut-h", window, cx);
        let sbar = scrollbar::scrollbar(
            "shortcut-scroll",
            Source::handle(&sh),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let q = self.filter.read(cx).value().trim().to_lowercase();
        let mut groups: Vec<(&str, Vec<(String, String)>)> = Vec::new();
        for c in actions::commands() {
            let keys = actions::bound_keys(&*(c.make)(), cx).join("  /  ");
            let hay = format!("{} {} {} {}", c.label, c.group, keys, c.keywords).to_lowercase();
            if keys.is_empty() || !(q.is_empty() || hay.contains(&q)) {
                continue;
            }
            match groups.iter_mut().find(|(g, _)| *g == c.group) {
                Some((_, rows)) => rows.push((c.label.to_owned(), keys)),
                None => groups.push((c.group, vec![(c.label.to_owned(), keys)])),
            }
        }
        frame(&theme, 640.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(
                &theme,
                "Keyboard shortcuts",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                body()
                    .child(self.filter.clone())
                    .child(
                        div()
                            .relative()
                            .h(px(420.))
                            .child(
                                div()
                                    .id("shortcut-list")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&sh)
                                    .flex()
                                    .flex_col()
                                    .gap(px(10.))
                                    .children(groups.into_iter().map(|(group, rows)| {
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .font_weight(gpui::FontWeight::MEDIUM)
                                                    .text_color(hsla(theme.muted))
                                                    .pb(px(4.))
                                                    .child(group.to_uppercase()),
                                            )
                                            .children(rows.into_iter().map(|(label, keys)| {
                                                div()
                                                    .flex()
                                                    .py(px(3.))
                                                    .child(div().flex_1().child(label))
                                                    .child(
                                                        div()
                                                            .text_color(hsla(theme.muted))
                                                            .child(keys),
                                                    )
                                            }))
                                    })),
                            )
                            .child(sbar),
                    )
                    .child(caption(
                        &theme,
                        "Change them in Options > Keyboard shortcuts.",
                    )),
            )
    }
}
