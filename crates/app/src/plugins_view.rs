// The Plugins window (IDA's Plugins list): every plugin the loader found in
// IDADIR and the IDAUSR directories, with its hotkey, flags and state. Listing
// loads nothing; Run loads the chosen plugin if needed and runs it with its
// argument. The user's own plugins are listed like any other and are only run
// when asked to be.
use crate::docs::DocEvent;
use crate::input::{InputEvent, TextInput};
use crate::plugins::{self, RunPlugin};
use crate::scrollbar::{self, Axes, Source};
use crate::session::Session;
use crate::table::{self, Col};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{PluginInfo, PluginList, plugin_flags};
use std::rc::Rc;

const COLS: [Col; 6] = [
    Col::fixed("Name", 230.).text(),
    Col::fixed("Hotkey", 130.),
    Col::fixed("Source", 64.).text(),
    Col::fixed("Flags", 150.),
    Col::fixed("State", 96.).text(),
    Col::flex("Path"),
];

pub struct PluginsView {
    session: Entity<Session>,
    shown: Rc<Vec<PluginInfo>>,
    selected: Option<(String, u64)>,
    show_hidden: bool,
    filter: Entity<TextInput>,
    loading: bool,
    message: Option<(SharedString, bool)>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DocEvent> for PluginsView {}

/// Asks the workspace to run a plugin (it owns the run, the messages and the
/// reload of the views).
pub struct RunRequested(pub RunPlugin);

impl EventEmitter<RunRequested> for PluginsView {}

impl PluginsView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextInput::new("", "Filter plugins", cx).with_icon("search"));
        let subs = vec![cx.subscribe(&filter, |this, _, _: &InputEvent, cx| this.refilter(cx))];
        let mut me = Self {
            session,
            shown: Rc::default(),
            selected: None,
            show_hidden: false,
            filter,
            loading: false,
            message: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subs: subs,
        };
        me.refilter(cx);
        me
    }

    /// Asks the host again (a plugin may have been added to a folder; IDA
    /// itself only scans when it starts, so this reflects what it has).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let fut = self.session.read(cx).call(PluginList {});
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                v.loading = false;
                match r {
                    Ok(list) => plugins::set_list(list, cx),
                    Err(e) => v.message = Some((format!("Listing failed: {e}").into(), true)),
                }
                v.refilter(cx);
            })
            .ok();
        })
        .detach();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.filter.read(cx).value().trim().to_lowercase();
        let hidden = self.show_hidden;
        self.shown = Rc::new(
            plugins::list()
                .into_iter()
                .filter(|p| hidden || plugins::runnable(p))
                .filter(|p| {
                    q.is_empty()
                        || format!("{} {} {}", p.name, p.path, p.hotkey)
                            .to_lowercase()
                            .contains(&q)
                })
                .collect(),
        );
        cx.notify();
    }

    fn run_selected(&mut self, cx: &mut Context<Self>) {
        let Some((path, arg)) = self.selected.clone() else {
            return;
        };
        cx.emit(RunRequested(RunPlugin { path, arg }));
    }

    pub fn set_message(
        &mut self,
        text: impl Into<SharedString>,
        error: bool,
        cx: &mut Context<Self>,
    ) {
        self.message = Some((text.into(), error));
        cx.notify();
    }

    fn render_rows(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let shown = self.shown.clone();
        let selected = self.selected.clone();
        let entity = cx.entity();
        uniform_list("plugin-rows", shown.len(), move |range, _w, _cx| {
            range
                .map(|ix| {
                    let p = &shown[ix];
                    let on = selected
                        .as_ref()
                        .is_some_and(|(path, arg)| *path == p.path && *arg == p.arg);
                    let key = (p.path.clone(), p.arg);
                    let entity = entity.clone();
                    let flags = plugin_flags::names(p.flags).join(" ");
                    let from_ida = plugins::from_ida_dir(p);
                    table::row_shell(("plugin", ix), on, !plugins::runnable(p), &theme)
                        .on_click(move |ev, _, cx| {
                            let key = key.clone();
                            entity.update(cx, |v, cx| {
                                v.selected = Some(key);
                                if ev.click_count() >= 2 {
                                    v.run_selected(cx);
                                }
                                cx.notify();
                            })
                        })
                        .child(table::cell(&COLS[0]).child(if p.arg == 0 {
                            p.name.clone()
                        } else {
                            format!("{} ({})", p.name, p.arg)
                        }))
                        .child(table::cell(&COLS[1]).child(p.hotkey.clone()))
                        .child(table::cell(&COLS[2]).child(if from_ida { "IDA" } else { "User" }))
                        .child(table::cell(&COLS[3]).child(flags))
                        .child(table::cell(&COLS[4]).child(if p.loaded {
                            ui::badge("loaded", BadgeKind::Success, &theme)
                        } else {
                            ui::badge("not loaded", BadgeKind::Neutral, &theme)
                        }))
                        .child(table::cell(&COLS[5]).child(p.path.clone()))
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1()
    }
}

impl Focusable for PluginsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PluginsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let total = plugins::list().len();
        let hidden_count = plugins::list()
            .iter()
            .filter(|p| !plugins::runnable(p))
            .count();
        let can_run = self.selected.is_some();
        let list = self.render_rows(theme, cx);
        let bar = scrollbar::scrollbar(
            "plugins-scroll",
            Source::list(&self.scroll),
            Axes::V,
            &theme,
            window,
            cx,
        );
        div()
            .id("plugins-view")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme.surface))
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(hsla(theme.border))
                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Plugins"))
                    .child(div().w(px(260.)).child(self.filter.clone()))
                    .child(
                        ui::segment("plugins-hidden", format!("Show hidden ({hidden_count})"), self.show_hidden, &theme).on_click(cx.listener(
                            |v, _, _, cx| {
                                v.show_hidden = !v.show_hidden;
                                v.refilter(cx);
                            },
                        )),
                    )
                    .child(div().flex_1())
                    .child(table::button("plugins-folder", "Open plugins folder", &theme).on_click(cx.listener(|_, _, _, cx| {
                        let dir = plugins::user_plugins_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        cx.reveal_path(&dir);
                    })))
                    .child(table::button("plugins-refresh", "Refresh", &theme).on_click(cx.listener(|v, _, _, cx| v.reload(cx))))
                    .child(
                        table::primary_button("plugins-run", "Run", &theme)
                            .when(!can_run, |b| b.opacity(0.45))
                            .on_click(cx.listener(|v, _, _, cx| v.run_selected(cx))),
                    ),
            )
            .child(ui::panel_header(format!("{} of {total} plugins", self.shown.len()), &theme))
            .child(table::header(&COLS, &theme))
            .child(div().relative().flex_1().min_h_0().flex().flex_col().child(list).child(bar))
            .child(
                div()
                    .h(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(12.))
                    .border_t_1()
                    .border_color(hsla(theme.border))
                    .bg(hsla(theme.surface2))
                    .text_size(px(11.5))
                    .font_family(code_font_name())
                    .text_color(hsla(match &self.message {
                        Some((_, true)) => theme.danger,
                        Some(_) => theme.success,
                        None => theme.muted,
                    }))
                    .child(match &self.message {
                        Some((m, _)) => m.clone(),
                        None => SharedString::from(
                            "Select a plugin and press Run (or double-click). Plugins that need IDA's graphical interface can't run here.",
                        ),
                    }),
            )
    }
}
