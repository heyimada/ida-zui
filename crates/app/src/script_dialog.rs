// File > Script file (Alt+F7): pick a Python file and run it in the console.
// The path is typed or pasted here, or chosen with Browse (the native picker
// cannot be automated, so it only sits behind that button).
use crate::actions::{Cancel, Confirm};
use crate::dialog::{ButtonKind, DialogEvent, body, button, caption, footer, frame, header};
use crate::input::{InputEvent, TextInput};
use crate::settings;
use crate::theme::{Theme, hsla};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, PathPromptOptions, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use std::path::PathBuf;

pub struct ScriptDialog {
    path: Entity<TextInput>,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for ScriptDialog {}

impl ScriptDialog {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let last = settings::get(cx).last_script.clone();
        let path = cx.new(|cx| {
            TextInput::new(&last, "Path of the .py file", cx)
                .tall()
                .mono()
        });
        let subs = vec![cx.subscribe(&path, |this, _, _: &InputEvent, cx| {
            this.error = None;
            cx.notify();
        })];
        Self {
            path,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self
            .path
            .read(cx)
            .value()
            .trim()
            .trim_matches('"')
            .to_owned();
        let path = PathBuf::from(&text);
        if text.is_empty() || !path.is_file() {
            self.error = Some(format!("'{text}' is not a file").into());
            cx.notify();
            return;
        }
        settings::update(cx, |s| s.last_script = text);
        cx.emit(DialogEvent::RunScript(path));
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a Python script".into()),
        });
        let input = self.path.clone();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(p) = paths.into_iter().next()
            {
                input.update(cx, |i, cx| i.set_value(&p.display().to_string(), cx));
            }
        })
        .detach();
    }
}

impl Focusable for ScriptDialog {
    /// The path field, so typing can start at once.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.path.read(cx).focus_handle(cx)
    }
}

impl Render for ScriptDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = theme;
        frame(&theme, 600.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|d, _: &Confirm, _, cx| d.submit(cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Close)))
            .child(header(&theme, "Run script file", cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))))
            .child(
                body()
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .child(div().flex_1().child(self.path.clone()))
                            .child(button("browse-script", "Browse…", ButtonKind::Secondary, &t).on_click(cx.listener(|d, _, window, cx| d.browse(window, cx)))),
                    )
                    .child(caption(&t, "The script runs in the Python console of this database. A traceback shows there."))
                    .when_some(self.error.clone(), |d, e| d.child(div().text_color(hsla(t.danger)).child(e))),
            )
            .child(
                footer(&theme)
                    .child(button("script-cancel", "Cancel", ButtonKind::Secondary, &t).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))))
                    .child(button("script-run", "Run", ButtonKind::Primary, &t).on_click(cx.listener(|d, _, _, cx| d.submit(cx)))),
            )
    }
}
