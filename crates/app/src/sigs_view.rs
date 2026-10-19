// The Signatures window (IDA's "Signatures" list): the FLIRT signatures in
// the database's list, the .sig files available for its processor, and
// applying one or several of them. Applying runs the analysis queues, so the
// session's progress shows in the status bar and in this view.
use crate::docs::DocEvent;
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Session, SessionEvent};
use crate::table::{self, Col};
use crate::theme::{Theme, code_font_name, hsla};
use crate::ui::{self, BadgeKind};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{SigApply, SigAvailable, SigList, SigState};
use std::collections::BTreeSet;
use std::rc::Rc;

const APPLIED: [Col; 4] = [
    Col::fixed("Name", 130.),
    Col::flex("Description").text(),
    Col::fixed("State", 90.),
    Col::fixed("Modules matched", 130.),
];
const AVAILABLE: [Col; 5] = [
    Col::fixed("", 34.),
    Col::fixed("Name", 130.),
    Col::flex("Description").text(),
    Col::fixed("Functions", 100.),
    Col::fixed("", 90.),
];

pub struct SigsView {
    session: Entity<Session>,
    state: Option<Rc<SigState>>,
    /// Available files that pass the filter, as indices into `state`.
    shown: Rc<Vec<SigAvailable>>,
    selected: BTreeSet<String>,
    filter: Entity<TextInput>,
    loading: bool,
    applying: bool,
    /// The outcome of the last request: text and whether it is an error.
    message: Option<(SharedString, bool)>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DocEvent> for SigsView {}

impl SigsView {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextInput::new("", "Filter signatures", cx).with_icon("search"));
        let subs = vec![
            cx.subscribe(&filter, |this, _, _: &InputEvent, cx| this.refilter(cx)),
            cx.subscribe(&session, |this, _, ev, cx| {
                if matches!(ev, SessionEvent::Changed) && !this.applying {
                    this.reload(cx);
                }
            }),
        ];
        let mut me = Self {
            session,
            state: None,
            shown: Rc::default(),
            selected: BTreeSet::new(),
            filter,
            loading: false,
            applying: false,
            message: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            _subs: subs,
        };
        me.reload(cx);
        me
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let fut = self.session.read(cx).call(SigList {});
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                v.loading = false;
                match r {
                    Ok(s) => v.set_state(s, cx),
                    Err(e) => v.message = Some((e.to_string().into(), true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_state(&mut self, state: SigState, cx: &mut Context<Self>) {
        // Applied files cannot be picked again.
        self.selected
            .retain(|n| state.available.iter().any(|a| a.name == *n && !a.applied));
        self.state = Some(Rc::new(state));
        self.refilter(cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let q = self.filter.read(cx).value().trim().to_lowercase();
        self.shown = Rc::new(
            self.state
                .as_ref()
                .map(|s| {
                    s.available
                        .iter()
                        .filter(|a| {
                            q.is_empty()
                                || a.name.to_lowercase().contains(&q)
                                || a.title.to_lowercase().contains(&q)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
        );
        cx.notify();
    }

    fn toggle(&mut self, name: &str, cx: &mut Context<Self>) {
        if !self.selected.remove(name) {
            self.selected.insert(name.to_owned());
        }
        cx.notify();
    }

    /// Plans the selected files and runs the analysis over the database.
    pub fn apply(&mut self, cx: &mut Context<Self>) {
        if self.applying || self.selected.is_empty() {
            return;
        }
        let names: Vec<String> = self.selected.iter().cloned().collect();
        self.applying = true;
        self.message = None;
        let fut = self.session.read(cx).call(SigApply {
            names: names.clone(),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = fut.await;
            this.update(cx, |v, cx| {
                v.applying = false;
                match r {
                    Ok(r) => {
                        let added = r.lib_after.saturating_sub(r.lib_before);
                        let mut text = format!(
                            "Applied {}: {} library function{} found ({} → {}).",
                            names.join(", "),
                            added,
                            if added == 1 { "" } else { "s" },
                            r.lib_before,
                            r.lib_after
                        );
                        if !r.failed.is_empty() {
                            text.push_str(&format!(" Not found: {}.", r.failed.join(", ")));
                        }
                        v.message = Some((text.into(), !r.failed.is_empty() && r.planned == 0));
                        v.selected.clear();
                        v.set_state(r.state, cx);
                        // Library functions get their badge in the lists and views.
                        v.session.update(cx, |s, cx| s.changed(cx));
                    }
                    Err(e) => v.message = Some((format!("Applying failed: {e}").into(), true)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_applied(
        &self,
        theme: Theme,
        state: &SigState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let ah = scrollbar::handle("applied-h", window, cx);
        let abar = scrollbar::scrollbar(
            "applied-scroll",
            Source::handle(&ah),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let rows = state.applied.iter().map(|a| {
            div()
                .id(("applied", a.index as usize))
                .h(px(table::ROW_HEIGHT))
                .w_full()
                .flex()
                .items_center()
                .text_size(px(12.))
                .child(table::cell(&APPLIED[0]).child(a.name.clone()))
                .child(table::cell(&APPLIED[1]).child(a.title.clone()))
                .child(table::cell(&APPLIED[2]).child(ui::badge(
                    a.state.clone(),
                    if a.state == "applied" {
                        BadgeKind::Success
                    } else {
                        BadgeKind::Neutral
                    },
                    &theme,
                )))
                .child(table::cell(&APPLIED[3]).child(a.matched.to_string()))
        });
        div()
            .flex_none()
            .child(ui::panel_header(
                format!("Applied to this database ({})", state.applied.len()),
                &theme,
            ))
            .child(table::header(&APPLIED, &theme))
            .child(
                div()
                    .relative()
                    .max_h(px(22. * 7.))
                    .child(
                        div()
                            .id("applied-rows")
                            .max_h(px(22. * 7.))
                            .overflow_y_scroll()
                            .track_scroll(&ah)
                            .children(rows),
                    )
                    .child(abar),
            )
    }

    fn render_available(
        &self,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let bar = scrollbar::scrollbar(
            "sig-scroll",
            Source::list(&self.scroll),
            Axes::V,
            &theme,
            window,
            cx,
        );
        let shown = self.shown.clone();
        let selected = self.selected.clone();
        let entity = cx.entity();
        let list = uniform_list("sig-rows", shown.len(), move |range, _w, _cx| {
            range
                .map(|ix| {
                    let a = &shown[ix];
                    let picked = selected.contains(&a.name);
                    let name = a.name.clone();
                    let entity = entity.clone();
                    let applied = a.applied;
                    table::row_shell(("sig", ix), picked, applied, &theme)
                        .when(!applied, |r| {
                            r.on_click(move |_, _, cx| {
                                let name = name.clone();
                                entity.update(cx, |v, cx| v.toggle(&name, cx))
                            })
                        })
                        .child(table::cell(&AVAILABLE[0]).child(check_box(picked, applied, &theme)))
                        .child(table::cell(&AVAILABLE[1]).child(a.name.clone()))
                        .child(table::cell(&AVAILABLE[2]).child(a.title.clone()))
                        .child(table::cell(&AVAILABLE[3]).child(if a.functions > 0 {
                            a.functions.to_string()
                        } else {
                            String::new()
                        }))
                        .child(table::cell(&AVAILABLE[4]).when(applied, |c| {
                            c.child(ui::badge("applied", BadgeKind::Success, &theme))
                        }))
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1();
        scrollbar::area(list, bar)
    }
}

fn check_box(on: bool, disabled: bool, theme: &Theme) -> impl IntoElement {
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(hsla(if on {
            theme.accent
        } else {
            theme.border_strong
        }))
        .bg(hsla(if on { theme.accent } else { theme.bg }))
        .when(disabled, |d| d.opacity(0.4))
        .when(on, |d| {
            d.child(ui::icon("check", 11., hsla(theme.on_accent)))
        })
}

impl Focusable for SigsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SigsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let state = self.state.clone();
        let progress = self
            .applying
            .then(|| self.session.read(cx).analysis.clone())
            .flatten();
        let n = self.selected.len();
        let apply_label = if n == 0 {
            "Apply".to_owned()
        } else {
            format!("Apply {n} selected")
        };
        let proc = state
            .as_ref()
            .map(|s| s.processor.clone())
            .unwrap_or_default();
        let libs = state.as_ref().map_or(0, |s| s.lib_functions);
        let busy = self.applying || self.loading;
        div()
            .id("sigs-view")
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
                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).child("Signatures"))
                    .child(div().w(px(260.)).child(self.filter.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .text_size(px(11.5))
                            .text_color(hsla(theme.muted))
                            .child(format!("processor {proc}"))
                            .child("·")
                            .child(format!("{libs} library functions")),
                    )
                    .child(div().flex_1())
                    .when(self.applying, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(11.5))
                                .text_color(hsla(theme.muted))
                                .child(ui::spinner("sig-spin", 14., hsla(theme.accent)))
                                .child(match &progress {
                                    Some(a) => format!("{} {}%", a.stage_label(), a.percent),
                                    None => "Matching signatures".to_owned(),
                                }),
                        )
                    })
                    .child(table::button("sig-refresh", "Refresh", &theme).on_click(cx.listener(|v, _, _, cx| v.reload(cx))))
                    .child(
                        table::primary_button("sig-apply", apply_label, &theme)
                            .when(n == 0 || busy, |b| b.opacity(0.45))
                            .on_click(cx.listener(|v, _, _, cx| v.apply(cx))),
                    ),
            )
            .when_some(state.clone(), |d, s| d.child(self.render_applied(theme, &s, window, cx)))
            .child(ui::panel_header(format!("Available for {proc} ({})", self.shown.len()), &theme).border_t_1())
            .child(table::header(&AVAILABLE, &theme))
            .child(self.render_available(theme, window, cx))
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
                        None => SharedString::from("Click rows to pick signature files, then Apply. Library functions get the lib badge."),
                    }),
            )
    }
}
