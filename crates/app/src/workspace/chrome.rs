// The window's chrome: title strip, toolbar, activity rail,
// editor tab strip, jump bar, pane headers, status bar and the welcome and
// loading screens. All of it is `impl Workspace`; the state lives in the
// parent module.
use super::*;
use crate::theme::{code_font_name, hsla};
use crate::ui::{self, icon, icon_button, tip};
use gpui::{AnyElement, WindowControlArea};
use std::time::Instant;

pub const TITLE_H: f32 = 32.;
pub const TOOLBAR_H: f32 = 44.;
pub const STATUS_H: f32 = 24.;
pub const RAIL_W: f32 = 44.;
pub const TAB_H: f32 = 32.;
pub const CRUMB_H: f32 = 28.;

/// `find.exe` from a path or a title.
pub(super) fn short_name(full: &str) -> String {
    full.split('(').next().unwrap_or(full).trim().to_owned()
}

/// "PE" / "ELF" / "Mach-O" / "Binary" from IDA's file type text.
fn format_label(file_type: &str) -> &'static str {
    let t = file_type.to_lowercase();
    if t.contains("(pe)") || t.contains("portable executable") {
        "PE"
    } else if t.contains("elf") {
        "ELF"
    } else if t.contains("mach-o") || t.contains("macho") {
        "Mach-O"
    } else {
        "Binary"
    }
}

pub fn arch_label(processor: &str, bitness: u32) -> String {
    match processor {
        "metapc" => {
            if bitness == 64 {
                "x86-64".to_owned()
            } else {
                "x86".to_owned()
            }
        }
        other => other.to_uppercase(),
    }
}

/// "18.0 KB" for the recent list.
fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024. && u < UNITS.len() - 1 {
        v /= 1024.;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

impl Workspace {
    /// The OS title strip. With databases open it carries one chip per
    /// database after the product glyph, Chrome style; the draggable area is
    /// the glyph and the space after the chips (the chips themselves are
    /// ordinary clickable client area).
    pub fn title_strip(
        &self,
        window: &Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = *theme;
        let maximized = window.is_maximized();
        let chips: Vec<_> = self
            .tabs(cx)
            .into_iter()
            .map(|i| {
                let d = &self.dbs[i];
                let s = d.session.read(cx);
                let elapsed = match s.phase {
                    Phase::Loading { started, .. } => Some(started.elapsed().as_secs()),
                    _ => None,
                };
                let path = s
                    .info
                    .as_ref()
                    .map(|i| i.input_path.clone())
                    .or_else(|| s.request.as_ref().map(|r| r.path.display().to_string()));
                let progress = settings::get(cx)
                    .show_analysis_progress
                    .then(|| s.analysis.as_ref().map(|a| (a.percent, a.stage_label())))
                    .flatten();
                (
                    d.id,
                    d.title(cx),
                    d.kind(cx),
                    d.dirty(cx),
                    i == self.active,
                    elapsed,
                    progress,
                    path.unwrap_or_default(),
                )
            })
            .collect();
        let control = |label: &'static str, name: &'static str, area: WindowControlArea| {
            div()
                .id(label)
                .window_control_area(area)
                .w(px(46.))
                .h(px(TITLE_H))
                .flex()
                .items_center()
                .justify_center()
                .text_color(hsla(t.muted))
                .cursor_pointer()
                .hover(move |d| {
                    if label == "Close" {
                        d.bg(hsla(t.danger)).text_color(hsla(t.bg))
                    } else {
                        d.bg(hsla(t.hover))
                    }
                })
                .child(icon(name, 16., hsla(t.muted)))
        };
        let glyph = ui::logo(16., &t);
        let left: AnyElement = if chips.is_empty() {
            div()
                .id("window-drag")
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .window_control_area(WindowControlArea::Drag)
                .child(
                    div()
                        .w(px(160.))
                        .pl(px(12.))
                        .flex()
                        .items_center()
                        .child(glyph),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .justify_center()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(hsla(t.muted))
                        .child("ida-zui"),
                )
                .child(div().w(px(160.)))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .child(
                    div()
                        .id("window-drag-glyph")
                        .h_full()
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .window_control_area(WindowControlArea::Drag)
                        .child(glyph),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .min_w_0()
                        .overflow_hidden()
                        .children(chips.into_iter().map(
                            |(id, name, kind, dirty, on, elapsed, progress, path)| {
                                self.db_chip(
                                    id, name, kind, dirty, on, elapsed, progress, path, &t, cx,
                                )
                            },
                        )),
                )
                .child(
                    div()
                        .id("window-drag")
                        .flex_1()
                        .h_full()
                        .window_control_area(WindowControlArea::Drag),
                )
                .into_any_element()
        };
        div()
            .h(px(TITLE_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(hsla(t.bg))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(left)
            .child(
                div()
                    .flex()
                    .child(
                        control("Minimize", "minimize", WindowControlArea::Min).on_click(
                            |_, window, cx| {
                                cx.stop_propagation();
                                window.minimize_window();
                            },
                        ),
                    )
                    .child(
                        control(
                            if maximized { "Restore" } else { "Maximize" },
                            "maximize",
                            WindowControlArea::Max,
                        )
                        .on_click(|_, window, cx| {
                            cx.stop_propagation();
                            window.zoom_window();
                        }),
                    )
                    .child(
                        control("Close", "close", WindowControlArea::Close).on_click(
                            |_, window, cx| {
                                cx.stop_propagation();
                                window.remove_window();
                            },
                        ),
                    ),
            )
    }

    /// One database in the strip: state icon (file, spinner while analysing,
    /// warning when the open failed), name, elapsed time while loading, the
    /// unsaved dot and a close button.
    #[allow(clippy::too_many_arguments)]
    fn db_chip(
        &self,
        id: u64,
        name: String,
        kind: TabKind,
        dirty: bool,
        on: bool,
        elapsed: Option<u64>,
        progress: Option<(u32, &'static str)>,
        path: String,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = *theme;
        let tip_text = match progress {
            Some((pct, stage)) => format!(
                "{} — {stage}, {pct}%",
                if path.is_empty() { name.clone() } else { path }
            ),
            None if path.is_empty() => name.clone(),
            None => path,
        };
        let state_icon: AnyElement = match kind {
            TabKind::Loading => {
                ui::spinner(("db-spin", id as usize), 14., hsla(t.accent)).into_any_element()
            }
            TabKind::Failed => icon("warning", 14., hsla(t.danger)).into_any_element(),
            _ if progress.is_some() => {
                ui::spinner(("db-spin", id as usize), 14., hsla(t.accent)).into_any_element()
            }
            _ => icon("file", 14., hsla(if on { t.accent } else { t.muted })).into_any_element(),
        };
        div()
            .id(("db-tab", id as usize))
            .h(px(24.))
            .max_w(px(220.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(8.))
            .pr(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_size(px(12.))
            .when(on, |d| {
                d.bg(hsla(t.accent_soft))
                    .text_color(hsla(t.text))
                    .font_weight(gpui::FontWeight::MEDIUM)
            })
            .when(!on, |d| {
                d.text_color(hsla(t.muted))
                    .hover(move |d| d.bg(hsla(t.hover)))
            })
            .tooltip(tip(tip_text, None))
            .on_click(cx.listener(move |w, _, window, cx| {
                if let Some(ix) = w.index_of(id) {
                    w.activate(ix, window, cx);
                }
            }))
            .child(state_icon)
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .when_some(progress, |d, (pct, _)| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(code_font_name())
                        .text_size(px(11.))
                        .text_color(hsla(t.accent))
                        .child(format!("{pct}%")),
                )
            })
            .when_some(elapsed, |d, secs| {
                d.child(
                    div()
                        .flex_none()
                        .font_family(code_font_name())
                        .text_size(px(11.))
                        .text_color(hsla(t.muted))
                        .child(format!("{}:{:02}", secs / 60, secs % 60)),
                )
            })
            .when(dirty, |d| d.child(ui::dot(6., t.warning)))
            .child(
                div()
                    .id(("db-close", id as usize))
                    .size(px(16.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .hover(move |d| d.bg(hsla(t.raised)))
                    .tooltip(tip(
                        "Close database",
                        Some(crate::actions::shortcut_text(&CloseDatabase, cx)),
                    ))
                    .on_click(cx.listener(move |w, _, window, cx| {
                        cx.stop_propagation();
                        w.close_db_checked(id, window, cx);
                    }))
                    .child(icon("close", 12., hsla(t.muted))),
            )
    }

    pub fn menu_row(&self, theme: &Theme) -> impl IntoElement + use<> {
        div()
            .h(px(crate::menubar::BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(px(8.))
            .bg(hsla(theme.bg))
            .border_b_1()
            .border_color(hsla(theme.border))
            .child(self.menubar.clone())
    }

    /// A toolbar icon button with a tooltip showing its shortcut.
    fn tool_button<A: gpui::Action + Default>(
        &self,
        id: &'static str,
        name: &'static str,
        label: &'static str,
        active: bool,
        enabled: bool,
        theme: &Theme,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        let shortcut = crate::actions::shortcut_text(&A::default(), cx);
        icon_button(id, name, 28., active, enabled, theme)
            .when(enabled, |d| d.tooltip(tip(label, Some(shortcut))))
    }

    /// The debugger's controls, shown while a process exists: continue or
    /// pause, the steps, run to cursor and terminate.
    fn debug_controls(&self, t: &Theme, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let (active, suspended, state, has_exception) = {
            let d = &self.session.read(cx).dbg;
            (d.active(), d.suspended(), d.state, d.exception.is_some())
        };
        if !active {
            return None;
        }
        let t = *t;
        // (id, icon, tooltip, enabled, colour, action)
        let button = |id: &'static str,
                      name: &'static str,
                      label: &'static str,
                      shortcut: String,
                      enabled: bool,
                      color: u32,
                      cx: &mut Context<Self>| {
            let b = div()
                .id(id)
                .size(px(26.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(move |d| d.bg(hsla(t.hover)))
                        .tooltip(tip(label, Some(shortcut)))
                })
                .child(icon(name, 16., hsla(if enabled { color } else { t.faint })));
            let _ = cx;
            b
        };
        let sc = |a: &dyn gpui::Action, cx: &App| crate::actions::shortcut_text(a, cx);
        let go =
            if suspended {
                button(
                    "dbg-continue",
                    "play",
                    "Continue",
                    sc(&DebugStart, cx),
                    true,
                    t.success,
                    cx,
                )
                .on_click(cx.listener(|w, _, _, cx| {
                    w.session.update(cx, |s, cx| s.dbg_start_or_continue(cx))
                }))
            } else {
                button(
                    "dbg-pause",
                    "pause",
                    "Pause",
                    sc(&DebugPause, cx),
                    true,
                    t.warning,
                    cx,
                )
                .on_click(cx.listener(|w, _, _, cx| w.session.update(cx, |s, cx| s.dbg_pause(cx))))
            };
        let step = |id: &'static str,
                    name: &'static str,
                    label: &'static str,
                    shortcut: String,
                    how: ida_proto::ResumeHow,
                    cx: &mut Context<Self>| {
            button(id, name, label, shortcut, suspended, t.text, cx).on_click(cx.listener(
                move |w, _, _, cx| {
                    let ea = w.session.read(cx).cursor;
                    w.session.update(cx, |s, cx| s.dbg_resume(how, ea, cx))
                },
            ))
        };
        let tag = match state {
            ida_proto::DbgRun::Running => ("Running", t.success),
            ida_proto::DbgRun::Suspended if has_exception => ("Exception", t.danger),
            _ => ("Suspended", t.warning),
        };
        Some(
            div()
                .h(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(2.))
                .px(px(4.))
                .rounded(px(8.))
                .bg(hsla(t.surface2))
                .border_1()
                .border_color(hsla(t.border))
                .child(go)
                .child(step(
                    "dbg-over",
                    "step-over",
                    "Step over",
                    sc(&DebugStepOver, cx),
                    ida_proto::ResumeHow::StepOver,
                    cx,
                ))
                .child(step(
                    "dbg-into",
                    "step-into",
                    "Step into",
                    sc(&DebugStepInto, cx),
                    ida_proto::ResumeHow::StepInto,
                    cx,
                ))
                .child(step(
                    "dbg-out",
                    "step-out",
                    "Run until return",
                    sc(&DebugStepOut, cx),
                    ida_proto::ResumeHow::StepOut,
                    cx,
                ))
                .child(step(
                    "dbg-to-cursor",
                    "run-to-cursor",
                    "Run to cursor",
                    sc(&DebugRunToCursor, cx),
                    ida_proto::ResumeHow::RunTo,
                    cx,
                ))
                .child(div().w(px(1.)).h(px(14.)).mx(px(2.)).bg(hsla(t.border)))
                .child(
                    button(
                        "dbg-stop",
                        "stop",
                        "Terminate process",
                        sc(&DebugTerminate, cx),
                        true,
                        t.danger,
                        cx,
                    )
                    .on_click(
                        cx.listener(|w, _, _, cx| {
                            w.session.update(cx, |s, cx| s.dbg_terminate(cx))
                        }),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .text_size(px(11.))
                        .text_color(hsla(t.muted))
                        .child(ui::dot(6., tag.1))
                        .child(tag.0),
                ),
        )
    }

    pub fn toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let s = self.session.read(cx);
        let ready = s.is_ready();
        let dirty = s.dirty;
        let (can_back, can_fwd) = (s.can_back(), s.can_forward());
        let file = s.file_name().unwrap_or_default();
        let detail = s
            .info
            .as_ref()
            .map(|i| {
                format!(
                    "{} {}",
                    format_label(&i.file_type),
                    arch_label(&i.processor, i.bitness)
                )
            })
            .unwrap_or_default();
        let sidebar_on = self.show_sidebar;
        let cluster = div()
            .w(px(244.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(
                self.tool_button::<ToggleSidebar>(
                    "tb-sidebar",
                    "sidebar",
                    "Toggle side panel",
                    sidebar_on,
                    ready,
                    &t,
                    cx,
                )
                .on_click(cx.listener(|w, _, _, cx| w.toggle_sidebar(cx))),
            )
            .child(
                self.tool_button::<Back>(
                    "tb-back",
                    "back",
                    "Back",
                    false,
                    ready && can_back,
                    &t,
                    cx,
                )
                .on_click(cx.listener(|w, _, _, cx| w.session.update(cx, |s, cx| s.back(cx)))),
            )
            .child(
                self.tool_button::<Forward>(
                    "tb-forward",
                    "forward",
                    "Forward",
                    false,
                    ready && can_fwd,
                    &t,
                    cx,
                )
                .on_click(cx.listener(|w, _, _, cx| w.session.update(cx, |s, cx| s.forward(cx)))),
            )
            .child(
                self.tool_button::<Save>("tb-save", "save", "Save database", false, ready, &t, cx)
                    .relative()
                    .on_click(cx.listener(|w, _, _, cx| w.session.update(cx, |s, cx| s.save(cx))))
                    .when(dirty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(4.))
                                .right(px(4.))
                                .child(ui::dot(6., t.warning)),
                        )
                    }),
            )
            .when(!ready, |d| {
                d.child(div().w(px(1.)).h(px(16.)).mx(px(4.)).bg(hsla(t.border)))
                    .child(
                        self.tool_button::<OpenFile>(
                            "tb-open",
                            "folder",
                            "Open file",
                            false,
                            true,
                            &t,
                            cx,
                        )
                        .on_click(cx.listener(|w, _, window, cx| w.prompt_open(window, cx))),
                    )
            });
        let analyzing = self.session.read(cx).analyzing();
        let pill = ready.then(|| {
            div()
                .h(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .rounded(px(6.))
                .bg(hsla(t.surface2))
                .border_1()
                .border_color(hsla(t.border))
                .text_size(px(12.))
                .child(icon("file", 16., hsla(t.muted)))
                .child(div().font_weight(gpui::FontWeight::MEDIUM).child(file))
                .child(div().text_color(hsla(t.muted)).child(detail))
                // Only while a run is in progress; idle is the normal state.
                .when(analyzing, |d| {
                    d.child(ui::spinner("pill-spin", 12., hsla(t.accent)))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(hsla(t.muted))
                                .child("Analyzing"),
                        )
                })
        });
        let debug = self.debug_controls(&t, cx);
        // The control shows the kind of the tab in front.
        let mode = match self.active_kind() {
            Some(crate::session::ViewKind::Listing) => Mode::Linear,
            Some(crate::session::ViewKind::Graph) => Mode::Graph,
            Some(crate::session::ViewKind::Hex) => Mode::Hex,
            Some(crate::session::ViewKind::Pseudocode) => Mode::Pseudo,
            None => self.mode,
        };
        let seg = |id: &'static str, label: &'static str, m: Mode, cx: &mut Context<Self>| {
            ui::segment(id, label, mode == m, &t)
                .on_click(cx.listener(move |w, _, window, cx| w.set_mode(m, window, cx)))
        };
        let segmented = ready.then(|| {
            ui::segmented_track(&t)
                .child(seg("seg-linear", "Linear", Mode::Linear, cx))
                .child(seg("seg-graph", "Graph", Mode::Graph, cx))
                .child(seg("seg-hex", "Hex", Mode::Hex, cx))
                .child(seg("seg-pseudo", "Pseudocode", Mode::Pseudo, cx))
        });
        let split = ready.then(|| {
            self.tool_button::<ToggleSplit>(
                "tb-split",
                "split",
                "Split with pseudocode",
                self.split,
                true,
                &t,
                cx,
            )
            .on_click(cx.listener(|w, _, window, cx| w.toggle_split(window, cx)))
        });
        let jump = ready.then(|| {
            div()
                .id("jump")
                .w(px(244.))
                .min_w(px(120.))
                .h(px(28.))
                .flex_shrink(1.)
                .overflow_hidden()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .rounded(px(6.))
                .bg(hsla(t.surface2))
                .border_1()
                .border_color(hsla(t.border))
                .text_size(px(12.))
                .text_color(hsla(t.faint))
                .cursor_pointer()
                // A launcher, not a second input: it opens the same jump modal as `G`.
                .tooltip(ui::tip(
                    "Jump to address or name",
                    Some(crate::actions::shortcut_text(&GoTo, cx)),
                ))
                .on_click(cx.listener(|w, _, window, cx| w.goto(window, cx)))
                .child(icon("search", 16., hsla(t.faint)))
                .child(div().flex_1().child("Jump to address or name"))
                .child(ui::key_cap(
                    crate::actions::shortcut_text(&GoTo, cx)
                        .split(" / ")
                        .next()
                        .unwrap_or("G")
                        .to_owned(),
                    &t,
                ))
        });
        let toggles = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .when(ready, |d| {
                d.child(
                    self.tool_button::<ToggleBottomPanel>(
                        "tb-bottom",
                        "panel-bottom",
                        "Toggle bottom panel",
                        self.show_bottom,
                        true,
                        &t,
                        cx,
                    )
                    .on_click(cx.listener(|w, _, _, cx| w.toggle_bottom(cx))),
                )
                .child(
                    self.tool_button::<ToggleInspector>(
                        "tb-inspector",
                        "panel-right",
                        "Toggle inspector",
                        self.show_inspector,
                        true,
                        &t,
                        cx,
                    )
                    .on_click(cx.listener(|w, _, _, cx| w.toggle_inspector(cx))),
                )
            })
            .when(!ready, |d| {
                d.child(
                    icon_button("tb-settings", "settings", 28., false, true, &t)
                        .tooltip(tip(
                            "Settings",
                            Some(crate::actions::shortcut_text(&OpenSettings, cx)),
                        ))
                        .on_click(cx.listener(|w, _, window, cx| {
                            w.open_settings(Section::Appearance, window, cx)
                        })),
                )
            });
        div()
            .h(px(TOOLBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .bg(hsla(t.bg))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(cluster)
            .children(pill)
            .children(debug)
            .child(div().flex_1())
            .children(segmented)
            .children(split)
            .child(div().flex_1())
            .children(jump)
            .child(toggles)
    }

    /// The activity rail (44 px): one button per list, settings at the bottom.
    pub fn rail(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let current = self.sidebar.read(cx).tab();
        let open = self.show_sidebar;
        div()
            .w(px(RAIL_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .py(px(8.))
            .gap(px(4.))
            .bg(hsla(t.bg))
            .border_r_1()
            .border_color(hsla(t.border))
            .children(crate::sidebar::TABS.iter().map(|&tab| {
                let on = open && tab == current;
                icon_button(("rail", tab as usize), tab.icon(), 32., on, true, &t)
                    .tooltip(tip(tab.label(), None))
                    .on_click(cx.listener(move |w, _, window, cx| {
                        let cur = w.sidebar.read(cx).tab();
                        if w.show_sidebar && cur == tab {
                            w.toggle_sidebar(cx);
                        } else {
                            w.show_tab(tab, window, cx);
                            w.save_layout(cx);
                        }
                    }))
            }))
            .child(div().flex_1())
            .child(
                icon_button("rail-settings", "settings", 32., false, true, &t)
                    .tooltip(tip(
                        "Settings",
                        Some(crate::actions::shortcut_text(&OpenSettings, cx)),
                    ))
                    .on_click(cx.listener(|w, _, window, cx| {
                        w.open_settings(Section::Appearance, window, cx)
                    })),
            )
    }

    /// The editor tab strip: the main view's tab, then the editors opened in tabs.
    pub fn tab_strip(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let s = self.session.read(cx);
        // The main view's own place (a pinned tab may have moved the session cursor).
        let main_at = match self.mode {
            crate::database::Mode::Linear => self.listing.read(cx).current_ea(),
            crate::database::Mode::Graph => self.graph.read(cx).current_ea(),
            crate::database::Mode::Hex => self.hex.read(cx).current_ea(),
            crate::database::Mode::Pseudo => self.pseudo.read(cx).current_ea(),
        }
        .unwrap_or(s.cursor);
        let main_title = s
            .function_at(main_at)
            .map(|f| short_name(&f.name))
            .or_else(|| s.file_name())
            .unwrap_or_default();
        let dirty = s.dirty;
        let main_on = self.doc.is_none();
        let docs: Vec<(u64, SharedString)> = self
            .docs
            .iter()
            .map(|d| match &d.view {
                crate::docs::DocView::Code(cv) => (d.id, self.code_tab_title(cv, cx)),
                _ => (d.id, d.title.clone()),
            })
            .collect();
        let tab_base = |id: gpui::ElementId, on: bool| {
            let tb = div()
                .id(id)
                .group("tab")
                .relative()
                .h(px(TAB_H))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .border_r_1()
                .border_color(hsla(t.border))
                .text_size(px(12.))
                .cursor_pointer()
                .text_color(hsla(if on { t.text } else { t.muted }));
            if on {
                tb.bg(hsla(t.surface))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(2.))
                            .bg(hsla(t.accent)),
                    )
            } else {
                tb.hover(move |d| d.bg(hsla(t.hover)))
            }
        };
        div()
            .h(px(TAB_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(hsla(t.bg))
            .border_b_1()
            .border_color(hsla(t.border))
            .child(
                tab_base("tab-main".into(), main_on)
                    .on_click(cx.listener(|w, _, window, cx| w.show_main(window, cx)))
                    .child(icon(
                        "view",
                        16.,
                        hsla(if main_on { t.accent } else { t.muted }),
                    ))
                    .child(main_title)
                    .when(dirty, |d| d.child(ui::dot(8., t.warning))),
            )
            .children(docs.into_iter().map(|(id, title)| {
                let on = self.doc == Some(id);
                tab_base(("doc-tab", id).into(), on)
                    .on_click(cx.listener(move |w, _, window, cx| w.show_doc(id, window, cx)))
                    .child(icon("view", 16., hsla(if on { t.accent } else { t.muted })))
                    .child(title)
                    .child(
                        div()
                            .id(("doc-close", id))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .hover(move |d| d.bg(hsla(t.raised)))
                            .on_click(cx.listener(move |w, _, window, cx| {
                                cx.stop_propagation();
                                w.close_doc(id, window, cx)
                            }))
                            .child(icon("close", 12., hsla(t.muted))),
                    )
            }))
    }

    /// `file › segment › function › address` with the block count on the right.
    pub fn jump_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let s = self.session.read(cx);
        let cursor = s.cursor;
        let file = s.file_name().unwrap_or_default();
        let func = s.function_at(cursor).map(|f| (f.name.clone(), f.start));
        let hexrays = s.info.as_ref().is_some_and(|i| i.hexrays);
        let focus = self.focus_ctx.read(cx);
        let segment = focus.addr.as_ref().and_then(|a| a.segment.clone());
        let blocks = focus.func.as_ref().map(|f| f.blocks);
        let chevron = || icon("chevron-right", 12., hsla(t.muted));
        div()
            .h(px(CRUMB_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .bg(hsla(t.surface))
            .border_b_1()
            .border_color(hsla(t.border))
            .text_size(px(12.))
            .child(div().text_color(hsla(t.muted)).child(file))
            .when_some(segment, |d, seg| {
                d.child(chevron())
                    .child(div().text_color(hsla(t.muted)).child(seg))
            })
            .when_some(func.clone(), |d, (name, start)| {
                d.child(chevron()).child(
                    div()
                        .id("crumb-func")
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .cursor_pointer()
                        .on_click(cx.listener(move |w, _, _, cx| {
                            w.session.update(cx, |s, cx| s.navigate(start, cx))
                        }))
                        .child(name),
                )
            })
            .child(chevron())
            .child(
                div()
                    .font_family(code_font_name())
                    .child(format!("0x{cursor:x}")),
            )
            .child(div().flex_1())
            .when_some(blocks, |d, n| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(hsla(t.muted))
                        .child(format!("{n} blocks")),
                )
            })
            .when(hexrays && func.is_some(), |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(hsla(t.muted))
                        .child("·  Hex-Rays decompiler"),
                )
            })
    }

    /// A pane header for the split view.
    pub fn pane_header(
        &self,
        title: &'static str,
        note: String,
        theme: &Theme,
    ) -> impl IntoElement + use<> {
        ui::panel_header(title, theme)
            .pr(px(12.))
            .child(div().flex_1())
            .child(ui::mono_note(note, theme))
    }

    pub fn status_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = *theme;
        let s = self.session.read(cx);
        let ready = s.is_ready();
        let cursor = s.cursor;
        let func = s
            .function_at(cursor)
            .map(|f| format!("{}+{:X}", short_name(&f.name), cursor - f.start));
        let status = s.status.get(Instant::now());
        let dirty = s.dirty;
        let caret = s.caret.clone();
        let info = s.info.clone();
        let analysis = s
            .analysis
            .clone()
            .filter(|_| settings::get(cx).show_analysis_progress);
        let busy = s.analysis.is_some();
        let segment = self
            .focus_ctx
            .read(cx)
            .addr
            .as_ref()
            .and_then(|a| a.segment.clone());
        let dbg_status = (s.dbg.active()).then(|| {
            let (dot, text) = match s.dbg.state {
                ida_proto::DbgRun::Running => (t.success, "Running".to_owned()),
                _ => {
                    let at = s.dbg.ip.map(|ip| s.symbol_at(ip)).unwrap_or_default();
                    match &s.dbg.exception {
                        Some(e) => (
                            t.danger,
                            format!(
                                "Exception {} at {at}",
                                if e.name.is_empty() {
                                    format!("0x{:08X}", e.code)
                                } else {
                                    e.name.clone()
                                }
                            ),
                        ),
                        None => (t.warning, format!("Suspended at {at} ({})", s.dbg.reason)),
                    }
                }
            };
            (dot, text)
        });
        // Only views with stable lines have a position worth showing; the
        // listing and hex view have none.
        let place = match (caret.block, caret.line) {
            (Some(b), Some(l)) => Some(format!("Block #{b}, line {l}")),
            (None, Some(l)) => Some(format!("Line {l}, Col {}", caret.col)),
            _ => None,
        };
        let bar = div()
            .h(px(STATUS_H))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(16.))
            .px(px(12.))
            .bg(hsla(t.bg))
            .border_t_1()
            .border_color(hsla(t.border))
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(hsla(t.muted));
        if !ready {
            let loading = matches!(s.phase, Phase::Loading { .. });
            let problem = ida_proto::client::HostConfig::discover().problem();
            let setup_problem = problem.is_some();
            return bar
                .child(div().child(if loading {
                    "Opening a database…"
                } else {
                    "No database open"
                }))
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(ui::dot(
                            6.,
                            if setup_problem { t.danger } else { t.success },
                        ))
                        .child(match problem {
                            Some(ida_proto::client::SetupProblem::HostMissing(_)) => {
                                "ida-host.exe missing"
                            }
                            Some(ida_proto::client::SetupProblem::IdaMissing) => "IDA not found",
                            None => "idalib ready",
                        }),
                );
        }
        bar.child(
            div()
                .font_family(code_font_name())
                .text_color(hsla(t.text))
                .child(format!("0x{cursor:x}")),
        )
        .when_some(func, |d, f| {
            d.child(div().font_family(code_font_name()).child(f))
        })
        .when_some(segment, |d, seg| d.child(div().child(seg)))
        .when_some(dbg_status, |d, (dot, text)| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(hsla(t.text))
                    .child(ui::dot(6., dot))
                    .child(text),
            )
        })
        .when(dirty, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(ui::dot(6., t.warning))
                    .child("Unsaved changes"),
            )
        })
        .child(div().flex_1())
        .when_some(status, |d, (msg, err)| {
            d.child(
                div()
                    .text_color(hsla(if err { t.danger } else { t.success }))
                    .child(msg),
            )
        })
        .when_some(place, |d, place| d.child(div().child(place)))
        .child(match analysis {
            Some(a) => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ui::spinner("status-spin", 12., hsla(t.accent)))
                .child(
                    div()
                        .text_color(hsla(t.text))
                        .child(format!("Analyzing {}%", a.percent)),
                )
                .child(a.stage_label())
                .into_any_element(),
            None if busy => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(ui::spinner("status-spin", 12., hsla(t.accent)))
                .child("Analyzing")
                .into_any_element(),
            None => div().into_any_element(),
        })
        .when_some(info, |d, i| {
            d.child(div().child(format!(
                "{} · {}",
                arch_label(&i.processor, i.bitness),
                format_label(&i.file_type)
            )))
        })
    }

    /// The welcome screen (S5): open buttons, a drop zone, recent files.
    pub fn welcome(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *theme;
        let wh = crate::scrollbar::handle("welcome-h", window, cx);
        let wbar = crate::scrollbar::scrollbar(
            "welcome-scroll",
            crate::scrollbar::Source::handle(&wh),
            crate::scrollbar::Axes::V,
            &t,
            window,
            cx,
        );
        let session = self.session.read(cx);
        let error = match &session.phase {
            Phase::Failed(e) => Some(e.clone()),
            _ => None,
        };
        let now = settings::unix_now();
        let recent: Vec<(String, u64, Option<u64>, String)> = settings::get(cx)
            .recent
            .iter()
            .filter(|r| std::path::Path::new(&r.path).exists())
            .map(|r| {
                (
                    r.path.clone(),
                    r.opened,
                    std::fs::metadata(&r.path).ok().map(|m| m.len()),
                    r.arch.clone(),
                )
            })
            .collect();
        let open_hint = crate::actions::shortcut_text(&OpenFile, cx);
        let problem = ida_proto::client::HostConfig::discover().problem();
        div()
            .size_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(hsla(t.surface))
            .child(
                div().relative().flex_1().min_h_0().child(
                div()
                    .id("welcome-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&wh)
                    .p(px(64.))
                    .pt(px(48.))
                    .child(
                        div()
                            .w(px(512.))
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .child(
                                        ui::logo(32., &t),
                                    )
                                    .child(div().text_size(px(20.)).line_height(px(28.)).font_weight(gpui::FontWeight::MEDIUM).child("ida-zui")),
                            )
                            .child(div().text_size(px(13.)).text_color(hsla(t.muted)).child("A modern interface for IDA Pro databases."))
                            .when_some(problem, |d, p| {
                                let locate = p == ida_proto::client::SetupProblem::IdaMissing;
                                d.child(
                                    div()
                                        .p(px(16.))
                                        .flex()
                                        .flex_col()
                                        .gap(px(10.))
                                        .rounded(px(8.))
                                        .border_1()
                                        .border_color(hsla(t.danger))
                                        .bg(hsla(t.danger_soft))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .text_color(hsla(t.danger))
                                                .child(icon("warning", 16., hsla(t.danger)))
                                                .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::MEDIUM).child(if locate { "IDA Pro was not found" } else { "ida-host.exe was not found" })),
                                        )
                                        .child(div().text_size(px(12.)).line_height(px(18.)).child(p.message()))
                                        .when(locate, |d| {
                                            d.child(div().text_size(px(12.)).line_height(px(18.)).text_color(hsla(t.muted)).child("ida-zui looks for a folder named IDA * with idalib.dll under Program Files. Pick yours, or set the IDADIR environment variable (it takes precedence)."))
                                                .child(
                                                    div().flex().child(
                                                        ui::button("locate-ida", "Locate IDA…", ui::ButtonKind::Primary, false, &t)
                                                            .on_click(cx.listener(|w, _, window, cx| w.locate_ida(window, cx))),
                                                    ),
                                                )
                                        }),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .child(
                                        ui::button("open", "Open…", ui::ButtonKind::Primary, false, &t)
                                            .gap(px(8.))
                                            .on_click(cx.listener(|w, _, window, cx| w.prompt_open(window, cx)))
                                            .child(div().font_family(code_font_name()).text_size(px(11.)).opacity(0.8).child(open_hint)),
                                    )
                                    .child(
                                        ui::button("open-options", "Open with options…", ui::ButtonKind::Secondary, false, &t)
                                            .on_click(cx.listener(|w, _, window, cx| w.open_options_dialog(None, window, cx))),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(96.))
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .justify_center()
                                    .gap(px(4.))
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_dashed()
                                    .border_color(hsla(t.border_strong))
                                    .bg(hsla(t.surface2))
                                    .child(icon("exports", 16., hsla(t.muted)))
                                    .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::MEDIUM).child("Drop a binary or .i64 here"))
                                    .child(div().text_size(px(11.)).text_color(hsla(t.muted)).child("PE, ELF, Mach-O, raw firmware or an existing database")),
                            )
                            .when_some(error, |d, e| {
                                d.child(
                                    div()
                                        .p(px(12.))
                                        .rounded(px(6.))
                                        .bg(hsla(t.danger_soft))
                                        .text_color(hsla(t.danger))
                                        .text_size(px(12.))
                                        .child(e),
                                )
                            })
                            .when(!recent.is_empty(), |d| {
                                d.child(
                                    div()
                                        .mt(px(16.))
                                        .flex()
                                        .flex_col()
                                        .child(div().px(px(12.)).pb(px(8.)).child(ui::section_label("Recent", &t)))
                                        .child(div().h(px(1.)).bg(hsla(t.border)))
                                        .children(recent.into_iter().enumerate().map(|(ix, (path, opened, size, arch))| {
                                            let p = std::path::Path::new(&path);
                                            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                            let dir = path.clone();
                                            div()
                                                .id(("recent", ix))
                                                .h(px(44.))
                                                .px(px(12.))
                                                .flex()
                                                .items_center()
                                                .gap(px(12.))
                                                .cursor_pointer()
                                                .hover(move |d| d.bg(hsla(t.hover)))
                                                .on_click(cx.listener(move |w, _, window, cx| w.open_path(PathBuf::from(&path), window, cx)))
                                                .child(icon("file", 16., hsla(t.muted)))
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .flex()
                                                        .flex_col()
                                                        .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::MEDIUM).child(name))
                                                        .child(
                                                            div()
                                                                .overflow_hidden()
                                                                .whitespace_nowrap()
                                                                .text_ellipsis()
                                                                .font_family(code_font_name())
                                                                .text_size(px(11.))
                                                                .text_color(hsla(t.faint))
                                                                .child(dir),
                                                        ),
                                                )
                                                .when(!arch.is_empty(), |d| d.child(ui::badge(arch.clone(), ui::BadgeKind::Neutral, &t)))
                                                .children(size.map(|b| div().flex_none().font_family(code_font_name()).text_size(px(11.)).text_color(hsla(t.muted)).child(size_label(b))))
                                                .child(div().flex_none().w(px(96.)).text_size(px(12.)).text_color(hsla(t.muted)).child(settings::ago(opened, now)))
                                        })),
                                )
                            }),
                    ),
            ).child(wbar)
            )
            .child(
                div()
                    .px(px(64.))
                    .pb(px(16.))
                    .font_family(code_font_name())
                    .text_size(px(11.))
                    .text_color(hsla(t.faint))
                    .child(format!("idalib · {}", ida_proto::client::HostConfig::discover().ida_dir.display())),
            )
            .into_any_element()
    }

    pub fn loading(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let t = *theme;
        let session = self.session.read(cx);
        let (name, secs) = match &session.phase {
            Phase::Loading { path, started } => (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                started.elapsed().as_secs(),
            ),
            _ => (String::new(), 0),
        };
        let progress = session
            .analysis
            .clone()
            .filter(|_| settings::get(cx).show_analysis_progress);
        let tail: Vec<SharedString> = session.log.iter().rev().take(14).rev().cloned().collect();
        let plugin_note = crate::loadlog::plugin_summary(&session.log);
        let show_output = self.show_ida_output;
        let elapsed = format!("{}:{:02} elapsed", secs / 60, secs % 60);
        // Progress bar: `raised` track, `accent` fill, 6 px.
        let bar = |pct: u32| {
            div()
                .w_full()
                .h(px(6.))
                .rounded(px(3.))
                .bg(hsla(t.raised))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(gpui::relative(pct as f32 / 100.))
                        .rounded(px(3.))
                        .bg(hsla(t.accent)),
                )
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.))
            .bg(hsla(t.surface))
            .child(
                div()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(format!("Analyzing {name}")),
            )
            .child(match progress {
                Some(a) => div()
                    .w(px(520.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(bar(a.percent))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(div().text_color(hsla(t.text)).child(a.stage_label()))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .font_family(code_font_name())
                                    .text_color(hsla(t.text))
                                    .child(format!("{}%", a.percent)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(format!("{} functions found", a.functions))
                            .child(div().flex_1())
                            .child(elapsed),
                    )
                    .into_any_element(),
                // Nothing reported yet: the file is still being loaded.
                None => div()
                    .w(px(520.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(bar(0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(if settings::get(cx).show_analysis_progress {
                                "Loading the file"
                            } else {
                                "Analyzing"
                            })
                            .child(div().flex_1())
                            .child(elapsed),
                    )
                    .into_any_element(),
            })
            .when_some(plugin_note, |d, note| {
                d.child(
                    div()
                        .mt(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(11.))
                        .text_color(hsla(t.muted))
                        .child(icon("info", 14., hsla(t.muted)))
                        .child(note),
                )
            })
            .child(
                ui::button(
                    "ida-output-toggle",
                    if show_output {
                        "Hide IDA output"
                    } else {
                        "Show IDA output"
                    },
                    ui::ButtonKind::Ghost,
                    true,
                    &t,
                )
                .on_click(cx.listener(|w, _, _, cx| {
                    w.show_ida_output = !w.show_ida_output;
                    cx.notify();
                })),
            )
            .when(show_output, |d| {
                d.child(
                    div()
                        .w(px(760.))
                        .h(px(280.))
                        .p(px(12.))
                        .rounded(px(6.))
                        .bg(hsla(t.surface2))
                        .border_1()
                        .border_color(hsla(t.border))
                        .overflow_hidden()
                        .font_family(code_font_name())
                        .text_size(px(12.))
                        .text_color(hsla(t.muted))
                        .flex()
                        .flex_col()
                        .justify_end()
                        .children(tail.into_iter().map(|l| div().whitespace_nowrap().child(l))),
                )
            })
            .into_any_element()
    }

    /// A thin draggable strip on one edge of a `relative` panel.
    pub fn resize_handle(
        &self,
        id: &'static str,
        which: Resize,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let h = div().id(id).absolute().on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |w, _, _, cx| {
                w.resizing = Some(which);
                cx.notify();
            }),
        );
        match which {
            Resize::Sidebar => h
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(4.))
                .cursor(gpui::CursorStyle::ResizeLeftRight),
            Resize::Inspector => h
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(4.))
                .cursor(gpui::CursorStyle::ResizeLeftRight),
            Resize::Bottom => h
                .left_0()
                .right_0()
                .top_0()
                .h(px(4.))
                .cursor(gpui::CursorStyle::ResizeUpDown),
        }
    }
}
