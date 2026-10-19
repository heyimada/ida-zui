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

pub struct AboutDialog {
    info: Option<Result<VersionInfo, SharedString>>,
    focus: FocusHandle,
}

impl EventEmitter<DialogEvent> for AboutDialog {}

impl AboutDialog {
    /// Asks the open database's host for its versions, or starts a throwaway
    /// host when nothing is open (the welcome screen has none).
    pub fn new(session: &Entity<Session>, cx: &mut Context<Self>) -> Self {
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
        cx.spawn(async move |this, cx| {
            let result = match fut {
                Some(f) => f.await,
                None => running.await,
            };
            if let Some(c) = transient {
                c.kill();
            }
            this.update(cx, |d, cx| {
                d.info = Some(result.map_err(|e| e.to_string().into()));
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
        let row = |label: &'static str, value: String| {
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
        match &self.info {
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
        frame(&theme, 640.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Confirm, _, cx| cx.emit(DialogEvent::Close)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(&theme, "ida-zui", cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))))
            .child(
                body()
                    .child(caption(&theme, "A modern front end for IDA Pro, built on zui. The IDA kernel runs headless through idalib."))
                    .child(div().flex().flex_col().gap(px(6.)).children(rows)),
            )
            .child(footer(&theme).child(button("close", "Close", ButtonKind::Primary, &theme).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)))))
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
