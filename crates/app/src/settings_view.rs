// Options > Settings: theme, fonts and the keyboard shortcut editor. Every
// change applies at once and is written to settings.json.
use crate::actions::{self, Cancel, Command, Confirm};
use crate::dialog::{ButtonKind, DialogEvent, button, caption, checkbox, footer, frame, header};
use crate::input::{InputEvent, TextInput};
use crate::scrollbar::{self, Axes, Source};
use crate::session::Session;
use crate::settings;
use crate::theme::{self, Theme, hsla};
use crate::ui::{self, icon};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Keystroke, SharedString,
    Subscription, Window, div, prelude::*, px,
};

const FONT_PRESETS: [&str; 5] = [
    "Cascadia Mono",
    "Consolas",
    "JetBrains Mono",
    "Fira Code",
    "Courier New",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Appearance,
    Listing,
    Keyboard,
    Analysis,
    Debugger,
    Python,
    About,
}

/// History sizes offered in Settings > Python.
const HISTORY_SIZES: [usize; 5] = [50, 200, 500, 1000, 5000];

/// A recorded key that collides with another command, waiting for a decision.
struct Pending {
    command: &'static Command,
    keys: String,
    other: &'static Command,
}

pub struct SettingsView {
    section: Section,
    font: Entity<TextInput>,
    filter: Entity<TextInput>,
    /// The Debugger section: program, arguments and working directory.
    dbg_path: Entity<TextInput>,
    dbg_args: Entity<TextInput>,
    dbg_cwd: Entity<TextInput>,
    /// The command whose shortcut is being recorded.
    recording: Option<&'static Command>,
    pending: Option<Pending>,
    interceptor: Option<Subscription>,
    /// The About section: IDA's versions, asked of the host once it is shown.
    session: Entity<Session>,
    about: Option<Result<ida_proto::VersionInfo, SharedString>>,
    about_asked: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for SettingsView {}

impl SettingsView {
    pub fn new(section: Section, session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let family = settings::get(cx).code_font.clone();
        let font = cx.new(|cx| TextInput::new(&family, "Font family", cx).tall().mono());
        let filter = cx.new(|cx| {
            TextInput::new("", "Search commands or keys", cx)
                .with_icon("search")
                .with_keycap("/")
        });
        let dbg = settings::get(cx).debugger.clone();
        let dbg_path = cx.new(|cx| {
            TextInput::new(&dbg.app_path, "The database input file", cx)
                .tall()
                .mono()
        });
        let dbg_args = cx.new(|cx| TextInput::new(&dbg.args, "No arguments", cx).tall().mono());
        let dbg_cwd = cx.new(|cx| {
            TextInput::new(&dbg.cwd, "The program folder", cx)
                .tall()
                .mono()
        });
        let subs = vec![
            cx.subscribe(&dbg_path, |_, input, _: &InputEvent, cx| {
                let v = input.read(cx).value().to_owned();
                settings::update(cx, |s| s.debugger.app_path = v);
            }),
            cx.subscribe(&dbg_args, |_, input, _: &InputEvent, cx| {
                let v = input.read(cx).value().to_owned();
                settings::update(cx, |s| s.debugger.args = v);
            }),
            cx.subscribe(&dbg_cwd, |_, input, _: &InputEvent, cx| {
                let v = input.read(cx).value().to_owned();
                settings::update(cx, |s| s.debugger.cwd = v);
            }),
            cx.subscribe(&font, |_, input, _: &InputEvent, cx| {
                let family = input.read(cx).value().trim().to_owned();
                if !family.is_empty() && family != settings::get(cx).code_font {
                    settings::update(cx, |s| s.code_font = family);
                }
                cx.notify();
            }),
            cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        let mut view = Self {
            section,
            font,
            filter,
            dbg_path,
            dbg_args,
            dbg_cwd,
            recording: None,
            pending: None,
            interceptor: None,
            session,
            about: None,
            about_asked: false,
            focus: cx.focus_handle(),
            _subs: subs,
        };
        view.ask_about(cx);
        view
    }

    /// The About section needs IDA's versions: ask the host the first time
    /// the section is shown.
    fn ask_about(&mut self, cx: &mut Context<Self>) {
        if self.section != Section::About || self.about_asked {
            return;
        }
        self.about_asked = true;
        let fut = crate::help::fetch_version(&self.session, cx);
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |v, cx| {
                v.about = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_section(&mut self, section: Section, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_recording();
        self.section = section;
        self.ask_about(cx);
        let focus = self.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    fn stop_recording(&mut self) {
        self.recording = None;
        self.pending = None;
        self.interceptor = None;
    }

    /// Grabs the next key combination for `command` ahead of every binding.
    fn start_recording(&mut self, command: &'static Command, cx: &mut Context<Self>) {
        self.stop_recording();
        self.recording = Some(command);
        let this = cx.entity().downgrade();
        self.interceptor = Some(cx.intercept_keystrokes(move |ev, _, cx| {
            let key = ev.keystroke.key.as_str();
            // Modifiers alone are not shortcuts; wait for the real key.
            if matches!(key, "shift" | "control" | "alt" | "platform" | "function") {
                return;
            }
            cx.stop_propagation();
            let k = Keystroke {
                modifiers: ev.keystroke.modifiers,
                key: ev.keystroke.key.clone(),
                key_char: None,
            };
            this.update(cx, |v, cx| v.recorded(k, cx)).ok();
        }));
        cx.notify();
    }

    fn recorded(&mut self, k: Keystroke, cx: &mut Context<Self>) {
        let Some(command) = self.recording else {
            return;
        };
        let plain = k.modifiers == Default::default();
        if plain && k.key == "escape" {
            self.stop_recording();
            cx.notify();
            return;
        }
        let keys = k.unparse();
        let user = settings::get(cx).keymap.clone();
        match actions::conflict(command.name, &keys, &user) {
            Some(other) => {
                self.pending = Some(Pending {
                    command,
                    keys,
                    other,
                });
                self.recording = None;
                self.interceptor = None;
            }
            None => self.assign(command, keys, None, cx),
        }
        cx.notify();
    }

    /// Binds `keys` to `command` alone, taking it away from `from`.
    fn assign(
        &mut self,
        command: &'static Command,
        keys: String,
        from: Option<&'static Command>,
        cx: &mut Context<Self>,
    ) {
        settings::update(cx, |s| {
            if let Some(other) = from {
                let rest: Vec<String> = actions::effective_keys(other, &s.keymap)
                    .into_iter()
                    .filter(|k| {
                        Keystroke::parse(k).map(|k| k.unparse()).ok().as_deref()
                            != Some(keys.as_str())
                    })
                    .collect();
                s.keymap.insert(other.name.to_owned(), rest);
            }
            s.keymap.insert(command.name.to_owned(), vec![keys.clone()]);
        });
        self.stop_recording();
        cx.notify();
    }

    fn reset(&mut self, command: &'static Command, cx: &mut Context<Self>) {
        settings::update(cx, |s| {
            s.keymap.remove(command.name);
        });
        cx.notify();
    }

    fn unbind(&mut self, command: &'static Command, cx: &mut Context<Self>) {
        settings::update(cx, |s| {
            s.keymap.insert(command.name.to_owned(), Vec::new());
        });
        cx.notify();
    }

    fn step_code_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        settings::update(cx, |s| {
            s.code_font_size = (s.code_font_size + delta).clamp(8., 36.)
        });
        cx.notify();
    }

    fn step_listing(&mut self, indent: bool, delta: i32, cx: &mut Context<Self>) {
        settings::update(cx, |s| {
            if indent {
                s.listing_indent = (s.listing_indent as i32 + delta).clamp(0, 32) as u32;
            } else {
                s.listing_comment = (s.listing_comment as i32 + delta).clamp(8, 96) as u32;
            }
        });
        cx.notify();
    }

    /// Everything back to the shipped defaults for the current section.
    fn restore_defaults(&mut self, cx: &mut Context<Self>) {
        self.stop_recording();
        match self.section {
            Section::Keyboard => settings::update(cx, |s| s.keymap.clear()),
            Section::Analysis => settings::update(cx, |s| {
                let d = settings::Settings::default();
                s.analysis_on_open = d.analysis_on_open;
                s.show_analysis_progress = d.show_analysis_progress;
            }),
            Section::Python => settings::update(cx, |s| {
                s.python_history_size = crate::py_history::DEFAULT_CAP
            }),
            Section::Debugger => {
                settings::update(cx, |s| s.debugger = settings::DebuggerSettings::default());
                for input in [&self.dbg_path, &self.dbg_args, &self.dbg_cwd] {
                    input.update(cx, |i, cx| i.set_value("", cx));
                }
            }
            Section::About => {}
            Section::Listing => settings::update(cx, |s| {
                s.listing_indent = settings::DEFAULT_INDENT;
                s.listing_comment = settings::DEFAULT_COMMENT_COLUMN;
            }),
            Section::Appearance => {
                settings::update(cx, |s| {
                    let d = settings::Settings::default();
                    s.theme = d.theme;
                    s.code_font = d.code_font;
                    s.code_font_size = d.code_font_size;
                    s.ui_font_size = d.ui_font_size;
                    s.compact_rows = d.compact_rows;
                });
                let family = settings::get(cx).code_font.clone();
                self.font.update(cx, |i, cx| i.set_value(&family, cx));
            }
        }
        cx.notify();
    }

    /// `Ctrl+Shift+P` as key caps joined by `+`.
    fn caps(keys: &str, theme: &Theme) -> gpui::Div {
        let t = *theme;
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .children(keys.split('+').enumerate().flat_map(move |(i, k)| {
                let mut parts = Vec::new();
                if i > 0 {
                    parts.push(
                        div()
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child("+")
                            .into_any_element(),
                    );
                }
                parts.push(ui::key_cap(k.to_owned(), &t).into_any_element());
                parts
            }))
    }

    fn render_appearance(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let t = *theme;
        let s = settings::get(cx).clone();
        let row = |label: &'static str, field: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(160.))
                        .flex_none()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_color(hsla(t.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let cards = Theme::ALL.iter().map(|c| {
            let on = c.name == s.theme().name;
            let c = *c;
            div()
                .id(c.name)
                .w(px(132.))
                .p(px(6.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .border_1()
                .border_color(hsla(if on { t.accent } else { t.border }))
                .when(on, |d| d.bg(hsla(t.accent_soft)))
                .when(!on, |d| d.hover(move |d| d.bg(hsla(t.hover))))
                .on_click(cx.listener(move |_, _, _, cx| {
                    settings::update(cx, |s| s.theme = c.name.to_owned());
                    cx.notify();
                }))
                .child(
                    div()
                        .p(px(6.))
                        .rounded(px(4.))
                        .bg(hsla(c.surface))
                        .border_1()
                        .border_color(hsla(c.border))
                        .font_family(theme::code_font_name())
                        .text_size(px(11.))
                        .flex()
                        .gap(px(5.))
                        .child(div().text_color(hsla(c.syntax.insn)).child("mov"))
                        .child(div().text_color(hsla(c.syntax.reg)).child("rax"))
                        .child(div().text_color(hsla(c.syntax.num)).child("1"))
                        .child(div().text_color(hsla(c.syntax.cmt)).child("; x")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(c.name),
                )
        });
        let stepper = |id: &'static str,
                       value: f32,
                       dec: fn(&mut Self, &mut Context<Self>),
                       inc: fn(&mut Self, &mut Context<Self>),
                       cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    ui::icon_button((id, 0usize), "minus", 28., false, true, &t)
                        .on_click(cx.listener(move |v, _, _, cx| dec(v, cx))),
                )
                .child(
                    div()
                        .w(px(48.))
                        .flex()
                        .justify_center()
                        .font_family(theme::code_font_name())
                        .text_size(px(12.))
                        .child(format!("{value} pt")),
                )
                .child(
                    ui::icon_button((id, 1usize), "plus", 28., false, true, &t)
                        .on_click(cx.listener(move |v, _, _, cx| inc(v, cx))),
                )
        };
        let presets = FONT_PRESETS.iter().map(|name| {
            let on = s.code_font.eq_ignore_ascii_case(name);
            ui::segment(*name, *name, on, &t).on_click(cx.listener(move |v, _, _, cx| {
                v.font.update(cx, |i, cx| i.set_value(name, cx));
            }))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(row(
                "Theme",
                div().flex().gap(px(8.)).children(cards).into_any_element(),
            ))
            .child(row("Code font", self.font.clone().into_any_element()))
            .child(row(
                "",
                ui::segmented_track(&t)
                    .h(px(30.))
                    .flex_wrap()
                    .children(presets)
                    .into_any_element(),
            ))
            .child(row(
                "Code font size",
                stepper(
                    "code-size",
                    s.code_font_size,
                    |v, cx| v.step_code_size(-1., cx),
                    |v, cx| v.step_code_size(1., cx),
                    cx,
                )
                .into_any_element(),
            ))
            .child(row(
                "Row density",
                ui::segmented_track(&t)
                    .children([false, true].map(|compact| {
                        ui::segment(
                            ("density", compact as usize),
                            if compact { "Compact" } else { "Comfortable" },
                            s.compact_rows == compact,
                            &t,
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            settings::update(cx, |s| s.compact_rows = compact);
                            cx.notify();
                        }))
                    }))
                    .into_any_element(),
            ))
            .child(row(
                "Preview",
                div()
                    .p(px(12.))
                    .rounded(px(6.))
                    .bg(hsla(t.surface))
                    .border_1()
                    .border_color(hsla(t.border))
                    .font_family(theme::code_font_name())
                    .text_size(px(theme::code_font_size()))
                    .flex()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_color(hsla(t.syntax.code_name))
                            .child("sub_140001000:"),
                    )
                    .child(div().text_color(hsla(t.syntax.insn)).child("mov"))
                    .child(div().text_color(hsla(t.syntax.reg)).child("rax,"))
                    .child(div().text_color(hsla(t.syntax.num)).child("0Ah"))
                    .child(div().text_color(hsla(t.syntax.cmt)).child("; sample"))
                    .into_any_element(),
            ))
    }

    fn render_listing(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let t = *theme;
        let s = settings::get(cx).clone();
        let row = |label: &'static str, field: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(160.))
                        .flex_none()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_color(hsla(t.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let stepper = |id: &'static str, value: u32, indent: bool, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    ui::icon_button((id, 0usize), "minus", 28., false, true, &t)
                        .on_click(cx.listener(move |v, _, _, cx| v.step_listing(indent, -1, cx))),
                )
                .child(
                    div()
                        .w(px(64.))
                        .flex()
                        .justify_center()
                        .font_family(theme::code_font_name())
                        .text_size(px(12.))
                        .child(format!("{value} cols")),
                )
                .child(
                    ui::icon_button((id, 1usize), "plus", 28., false, true, &t)
                        .on_click(cx.listener(move |v, _, _, cx| v.step_listing(indent, 1, cx))),
                )
        };
        let sample = |indent: u32, comment: u32| {
            let pad = |n: u32| " ".repeat(n as usize);
            let line = format!("{}mov{}rax, [rcx+8]", pad(indent), pad(1));
            let width = line.chars().count() as u32;
            format!("{line}{}; load", pad(comment.saturating_sub(width).max(1)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(row("Instruction indent", stepper("indent", s.listing_indent, true, cx).into_any_element()))
            .child(row("Comment column", stepper("comment", s.listing_comment, false, cx).into_any_element()))
            .child(row(
                "Preview",
                div()
                    .p(px(12.))
                    .rounded(px(6.))
                    .bg(hsla(t.surface))
                    .border_1()
                    .border_color(hsla(t.border))
                    .font_family(theme::code_font_name())
                    .text_size(px(theme::code_font_size()))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .child(div().text_color(hsla(t.syntax.code_name)).child("loc_140001000:"))
                    .child(div().text_color(hsla(t.text)).child(sample(s.listing_indent, s.listing_comment)))
                    .into_any_element(),
            ))
            .child(row(
                "",
                div()
                    .text_size(px(11.))
                    .text_color(hsla(t.faint))
                    .child("Narrow values keep operands visible in the split view and in graph blocks. Applied to every open database at once.")
                    .into_any_element(),
            ))
    }

    fn render_analysis(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let t = *theme;
        let s = settings::get(cx).clone();
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::MEDIUM).child("Analysis"))
            .child(
                checkbox("analysis-on-open", "Run auto-analysis when a file is opened", s.analysis_on_open, &t).on_click(cx.listener(|_, _, _, cx| {
                    settings::update(cx, |s| s.analysis_on_open = !s.analysis_on_open);
                    cx.notify();
                })),
            )
            .child(caption(&t, "Opening a database that already exists never re-analyses it. Open with options can still turn analysis off for one file."))
            .child(
                checkbox("analysis-progress", "Show analysis progress", s.show_analysis_progress, &t).on_click(cx.listener(|_, _, _, cx| {
                    settings::update(cx, |s| s.show_analysis_progress = !s.show_analysis_progress);
                    cx.notify();
                })),
            )
            .child(caption(&t, "The loading screen, the database tab and the status bar show the stage and a percentage; off leaves the elapsed time only."))
    }

    fn render_debugger(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let t = *theme;
        let d = settings::get(cx).debugger.clone();
        let row = |label: &'static str, field: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(160.))
                        .flex_none()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_color(hsla(t.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let policies = settings::ExceptionPolicy::ALL
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                ui::segment(("exc-policy", i), p.label(), d.exceptions == p, &t).on_click(
                    cx.listener(move |_, _, _, cx| {
                        settings::update(cx, |s| s.debugger.exceptions = p);
                        cx.notify();
                    }),
                )
            });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(div().text_size(px(15.)).font_weight(gpui::FontWeight::MEDIUM).child("Debugger"))
            .child(row("Application", self.dbg_path.clone().into_any_element()))
            .child(row("Arguments", self.dbg_args.clone().into_any_element()))
            .child(row("Working directory", self.dbg_cwd.clone().into_any_element()))
            .child(caption(&t, "Leave the application empty to debug the database's input file. The debugger runs real code: only start programs you trust."))
            .child(
                checkbox("dbg-entry", "Stop at the process entry point", d.stop_at_entry, &t).on_click(cx.listener(|_, _, _, cx| {
                    settings::update(cx, |s| s.debugger.stop_at_entry = !s.debugger.stop_at_entry);
                    cx.notify();
                })),
            )
            .child(
                checkbox("dbg-lib", "Stop when a library is loaded or unloaded", d.stop_on_library, &t).on_click(cx.listener(|_, _, _, cx| {
                    settings::update(cx, |s| s.debugger.stop_on_library = !s.debugger.stop_on_library);
                    cx.notify();
                })),
            )
            .child(
                checkbox("dbg-thread", "Stop when a thread starts or exits", d.stop_on_thread, &t).on_click(cx.listener(|_, _, _, cx| {
                    settings::update(cx, |s| s.debugger.stop_on_thread = !s.debugger.stop_on_thread);
                    cx.notify();
                })),
            )
            .child(
                checkbox("dbg-aslr", "Disable ASLR for the process", d.disable_aslr, &t).on_click(cx.listener(|_, _, _, cx| {
                    settings::update(cx, |s| s.debugger.disable_aslr = !s.debugger.disable_aslr);
                    cx.notify();
                })),
            )
            .child(caption(&t, "The database is moved to the image base the system chose while the process lives and moved back when it ends, so it is never saved rebased."))
            .child(row(
                "Exceptions",
                ui::segmented_track(&t).children(policies).into_any_element(),
            ))
            .child(caption(&t, "Applied when a process starts. The Exceptions panel changes single codes for the open database."))
    }

    fn render_about(&self, theme: &Theme) -> gpui::Div {
        let t = *theme;
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("About ida-zui"),
            )
            .child(caption(&t, crate::help::ABOUT_BLURB))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .children(crate::help::about_rows(&t, self.about.as_ref())),
            )
    }

    fn render_python(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let t = *theme;
        let s = settings::get(cx).clone();
        let stored = crate::py_console::history_len(cx);
        let row = |label: &'static str, field: gpui::AnyElement| {
            div()
                .flex()
                .items_start()
                .gap(px(8.))
                .child(
                    div()
                        .w(px(160.))
                        .flex_none()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .justify_end()
                        .text_color(hsla(t.muted))
                        .child(label),
                )
                .child(div().flex_1().min_w_0().child(field))
        };
        let sizes = HISTORY_SIZES.iter().map(|&n| {
            ui::segment(
                ("history-size", n),
                n.to_string(),
                s.python_history_size == n,
                &t,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                settings::update(cx, |s| s.python_history_size = n);
                cx.notify();
            }))
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Python console"),
            )
            .child(row(
                "History size",
                ui::segmented_track(&t).children(sizes).into_any_element(),
            ))
            .child(row(
                "Console font",
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_family(theme::code_font_name())
                            .child(format!("{} {} pt", s.code_font, s.code_font_size)),
                    )
                    .child(caption(&t, "The code font from Appearance."))
                    .into_any_element(),
            ))
            .child(row(
                "Saved history",
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(div().child(format!(
                        "{stored} command{}",
                        if stored == 1 { "" } else { "s" }
                    )))
                    .child(
                        ui::button(
                            "clear-history",
                            "Clear history",
                            ButtonKind::Secondary,
                            true,
                            &t,
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            crate::py_console::clear_history(cx);
                            cx.notify();
                        })),
                    )
                    .into_any_element(),
            ))
            .child(row(
                "",
                caption(
                    &t,
                    format!("Kept in {}.", crate::py_console::history_path().display()),
                )
                .into_any_element(),
            ))
    }

    fn render_keyboard(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let t = *theme;
        let kh = scrollbar::handle("keymap-h", window, cx);
        let kbar = scrollbar::scrollbar(
            "keymap-scroll",
            Source::handle(&kh),
            Axes::V,
            theme,
            window,
            cx,
        );
        let q = self.filter.read(cx).value().trim().to_lowercase();
        let user = settings::get(cx).keymap.clone();
        let mut rows = Vec::new();
        let mut last_group = "";
        let mut shown = 0;
        for (ix, c) in actions::commands().into_iter().enumerate() {
            let keys: Vec<String> = actions::effective_keys(c, &user)
                .iter()
                .map(|k| actions::pretty_str(k))
                .collect();
            let hay =
                format!("{} {} {} {}", c.label, c.group, keys.join(" "), c.keywords).to_lowercase();
            if !q.is_empty() && !hay.contains(&q) {
                continue;
            }
            shown += 1;
            if c.group != last_group {
                last_group = c.group;
                rows.push(
                    div()
                        .h(px(28.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .text_size(px(11.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(hsla(t.muted))
                        .child(c.group.to_uppercase())
                        .into_any_element(),
                );
            }
            let recording = self.recording.is_some_and(|r| r.name == c.name);
            let changed = user.contains_key(c.name);
            let command: &'static Command = c;
            let bound = !keys.is_empty();
            let group = SharedString::from(format!("key-row-{ix}"));
            rows.push(
                div()
                    .id(("key-row", ix))
                    .group(group.clone())
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .when(changed || recording, |d| d.bg(hsla(t.hover)))
                    .when(!(changed || recording), |d| {
                        d.hover(move |d| d.bg(hsla(t.hover)))
                    })
                    .on_click(cx.listener(move |v, _, _, cx| v.start_recording(command, cx)))
                    .child(
                        div()
                            .w(px(260.))
                            .flex_none()
                            .text_size(px(13.))
                            .child(c.label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(if recording {
                                div()
                                    .h(px(24.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(hsla(t.accent))
                                    .bg(hsla(t.bg))
                                    .shadow(ui::ring(&t))
                                    .text_size(px(12.))
                                    .child(ui::dot(6., t.accent))
                                    .child(
                                        div()
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .child("Press keys…"),
                                    )
                                    .child(div().text_color(hsla(t.muted)).child("Esc cancels"))
                                    .into_any_element()
                            } else if !bound {
                                div()
                                    .text_size(px(12.))
                                    .text_color(hsla(t.faint))
                                    .child("not bound")
                                    .into_any_element()
                            } else {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .children(keys.iter().enumerate().flat_map(|(i, k)| {
                                        let mut parts = Vec::new();
                                        if i > 0 {
                                            parts.push(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(hsla(t.muted))
                                                    .child("or")
                                                    .into_any_element(),
                                            );
                                        }
                                        parts.push(Self::caps(k, &t).into_any_element());
                                        parts
                                    }))
                                    .into_any_element()
                            }),
                    )
                    .when(changed, |d| {
                        d.child(
                            ui::icon_button(("reset", ix), "undo", 24., false, true, &t).on_click(
                                cx.listener(move |v, _, _, cx| {
                                    cx.stop_propagation();
                                    v.reset(command, cx);
                                }),
                            ),
                        )
                    })
                    .when(bound && !recording, |d| {
                        d.child(
                            ui::icon_button(("unbind", ix), "close", 24., false, true, &t)
                                .invisible()
                                .group_hover(group.clone(), |d| d.visible())
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    cx.stop_propagation();
                                    v.unbind(command, cx);
                                })),
                        )
                    })
                    .into_any_element(),
            );
        }
        let modified = user.len();
        let warning = self.pending.as_ref().map(|p| {
            let (command, other, keys) = (p.command, p.other, p.keys.clone());
            div()
                .mx(px(8.))
                .my(px(4.))
                .h(px(40.))
                .px(px(12.))
                .rounded(px(6.))
                .bg(hsla(t.warning_soft))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(icon("warning", 16., hsla(t.warning)))
                .child(div().flex_1().text_size(px(12.)).child(format!(
                    "{} is also used by {}. Assigning it here removes it from there.",
                    actions::pretty_str(&keys),
                    other.label
                )))
                .child(
                    ui::button(
                        "assign-anyway",
                        "Assign anyway",
                        ButtonKind::Secondary,
                        true,
                        &t,
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.assign(command, keys.clone(), Some(other), cx);
                    })),
                )
                .child(
                    ui::button("assign-cancel", "Cancel", ButtonKind::Ghost, true, &t).on_click(
                        cx.listener(|v, _, _, cx| {
                            v.stop_recording();
                            cx.notify();
                        }),
                    ),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .child("Keyboard"),
                            )
                            .child(caption(
                                &t,
                                "Select a shortcut to record a new key combination.",
                            )),
                    )
                    .child(div().w(px(280.)).child(self.filter.clone())),
            )
            .child(
                div()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(hsla(t.border))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(24.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .bg(hsla(t.surface2))
                            .border_b_1()
                            .border_color(hsla(t.border))
                            .text_size(px(11.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(hsla(t.muted))
                            .child(div().w(px(268.)).child("Command"))
                            .child("Shortcut"),
                    )
                    .when_some(warning, |d, w| d.child(w))
                    .child(
                        div()
                            .relative()
                            .h(px(340.))
                            .child(
                                div()
                                    .id("keymap-list")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&kh)
                                    .flex()
                                    .flex_col()
                                    .children(rows),
                            )
                            .child(kbar),
                    )
                    .child(
                        div()
                            .h(px(24.))
                            .px(px(12.))
                            .flex()
                            .items_center()
                            .border_t_1()
                            .border_color(hsla(t.border))
                            .bg(hsla(t.surface2))
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(format!("{shown} commands  ·  {modified} modified")),
                    ),
            )
    }
}

impl Focusable for SettingsView {
    /// The filter box when the shortcut list is showing, so typing filters.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.section {
            Section::Keyboard => self.filter.read(cx).focus_handle(cx),
            Section::Appearance
            | Section::Listing
            | Section::Analysis
            | Section::Debugger
            | Section::Python
            | Section::About => self.focus.clone(),
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let t = theme;
        let nav = |id: &'static str,
                   label: &'static str,
                   name: &'static str,
                   section: Option<Section>,
                   cx: &mut Context<Self>| {
            let on = section == Some(self.section);
            let enabled = section.is_some();
            let el = div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(4.))
                .text_size(px(12.))
                .font_weight(if on {
                    gpui::FontWeight::MEDIUM
                } else {
                    gpui::FontWeight::NORMAL
                })
                .text_color(hsla(if !enabled {
                    t.faint
                } else if on {
                    t.text
                } else {
                    t.muted
                }))
                .when(on, |d| d.bg(hsla(t.accent_soft)))
                .when(enabled && !on, |d| {
                    d.cursor_pointer().hover(move |d| d.bg(hsla(t.hover)))
                })
                .child(icon(
                    name,
                    16.,
                    hsla(if !enabled {
                        t.faint
                    } else if on {
                        t.accent
                    } else {
                        t.muted
                    }),
                ))
                .child(label);
            match section {
                Some(section) => el
                    .on_click(
                        cx.listener(move |v, _, window, cx| v.set_section(section, window, cx)),
                    )
                    .into_any_element(),
                None => el.into_any_element(),
            }
        };
        let body_el = match self.section {
            Section::Appearance => self.render_appearance(&theme, cx),
            Section::Listing => self.render_listing(&theme, cx),
            Section::Keyboard => self.render_keyboard(&theme, window, cx),
            Section::Analysis => self.render_analysis(&theme, cx),
            Section::Debugger => self.render_debugger(&theme, cx),
            Section::Python => self.render_python(&theme, cx),
            Section::About => self.render_about(&theme),
        };
        let restore = "Restore defaults";
        frame(&theme, 920.)
            .key_context("Modal")
            .track_focus(&self.focus)
            .on_action(cx.listener(|v, _: &Cancel, _, cx| {
                // Escape leaves recording first, then closes.
                if v.recording.is_some() || v.pending.is_some() {
                    v.stop_recording();
                    cx.notify();
                } else {
                    cx.emit(DialogEvent::Close);
                }
            }))
            .on_action(cx.listener(|_, _: &Confirm, _, _| {}))
            .child(header(
                &theme,
                "Settings",
                cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close)),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .w(px(200.))
                            .flex_none()
                            .p(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .border_r_1()
                            .border_color(hsla(t.border))
                            .child(nav(
                                "nav-appearance",
                                "Appearance",
                                "contrast",
                                Some(Section::Appearance),
                                cx,
                            ))
                            .child(nav(
                                "nav-listing",
                                "Listing",
                                "strings",
                                Some(Section::Listing),
                                cx,
                            ))
                            .child(nav(
                                "nav-keyboard",
                                "Keyboard",
                                "keyboard",
                                Some(Section::Keyboard),
                                cx,
                            ))
                            .child(nav(
                                "nav-analysis",
                                "Analysis",
                                "bars",
                                Some(Section::Analysis),
                                cx,
                            ))
                            .child(nav(
                                "nav-debugger",
                                "Debugger",
                                "debug",
                                Some(Section::Debugger),
                                cx,
                            ))
                            .child(nav(
                                "nav-python",
                                "Python",
                                "code",
                                Some(Section::Python),
                                cx,
                            ))
                            .child(nav("nav-about", "About", "info", Some(Section::About), cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p(px(16.))
                            .px(px(24.))
                            .child(body_el),
                    ),
            )
            .child(
                footer(&theme)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(hsla(t.muted))
                            .child(format!("Saved to {}", settings::settings_path().display())),
                    )
                    .child(
                        button("restore", restore, ButtonKind::Ghost, &t)
                            .on_click(cx.listener(|v, _, _, cx| v.restore_defaults(cx))),
                    )
                    .child(
                        button("done", "Done", ButtonKind::Primary, &t)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
                    ),
            )
    }
}
