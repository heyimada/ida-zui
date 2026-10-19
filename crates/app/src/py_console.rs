// The IDAPython console: a multi-line input (zui-editor in console mode) over
// an output list. Enter runs the statement once it is complete (the host
// decides, with Python's own `codeop`), Shift+Enter adds a line, Ctrl+Enter
// runs regardless, Up and Down on the first and last line walk the history.
// Output is told apart by stream: stdout, stderr, the printed value of an
// expression and tracebacks. The host cannot serve other requests while a
// script runs, so a running script shows a strip with elapsed time and a
// Cancel button that raises KeyboardInterrupt in it.
use crate::py_history::History;
use crate::scrollbar::{self, Axes, Source};
use crate::session::{Phase, Session};
use crate::settings;
use crate::theme::{Theme, code_font_name, code_font_size, code_line_height, hsla};
use crate::ui::{self, ButtonKind, spinner};
use gpui::{
    App, Context, Entity, Global, IntoElement, Render, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{Interrupt, PyEval, PyReply, PyRunFile};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use zui_editor::{Editor, EditorEvent, PythonHighlighter};

/// Output kept in the list; older lines fall off the top.
const MAX_LINES: usize = 20_000;
/// The input grows with its text up to this many lines, then scrolls.
const MAX_INPUT_ROWS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// An echoed input line; the flag marks the first line of a command.
    Input(bool),
    Out,
    Err,
    /// The printed value of a trailing expression.
    Result,
    Traceback,
    /// The console's own remarks.
    Note,
}

#[derive(Clone, Debug)]
struct Line {
    kind: Kind,
    text: String,
    /// More text for this line may follow (output without a trailing newline).
    open: bool,
}

struct Running {
    started: Instant,
    /// A script file rather than console input.
    file: bool,
}

pub struct PyConsole {
    session: Entity<Session>,
    input: Entity<Editor>,
    lines: Rc<Vec<Line>>,
    scroll: UniformListScrollHandle,
    running: Option<Running>,
    history: Rc<RefCell<History>>,
    /// Theme and code font the input was last styled with.
    style: (&'static str, SharedString, u32),
    _subs: Vec<Subscription>,
}

struct HistoryStore(Rc<RefCell<History>>);

impl Global for HistoryStore {}

pub fn history_path() -> PathBuf {
    settings::config_dir().join("python_history.json")
}

/// The history every console shares; read from disk on first use.
fn history(cx: &mut App) -> Rc<RefCell<History>> {
    if !cx.has_global::<HistoryStore>() {
        let cap = settings::get(cx).python_history_size;
        cx.set_global(HistoryStore(Rc::new(RefCell::new(History::load(
            &history_path(),
            cap,
        )))));
    }
    cx.global::<HistoryStore>().0.clone()
}

/// Settings > Python > Clear history.
pub fn clear_history(cx: &mut App) {
    history(cx).borrow_mut().clear();
    let _ = std::fs::remove_file(history_path());
}

pub fn history_len(cx: &mut App) -> usize {
    history(cx).borrow().len()
}

impl PyConsole {
    pub fn new(session: Entity<Session>, cx: &mut Context<Self>) -> Self {
        let theme = Theme::get(cx);
        let input = cx.new(|cx| {
            let mut e = Editor::new(cx)
                .with_highlighter(PythonHighlighter)
                .with_theme(theme.editor())
                .with_font(
                    code_font_name(),
                    px(code_font_size()),
                    px(code_line_height()),
                )
                .with_gutter(false);
            e.set_console_mode(true);
            e.set_indent_openers("{:");
            e
        });
        let subs = vec![
            cx.subscribe(&input, |this, _, ev: &EditorEvent, cx| {
                this.on_editor(ev, cx)
            }),
            cx.observe(&session, |_, _, cx| cx.notify()),
        ];
        let history = history(cx);
        Self {
            session,
            input,
            lines: Rc::default(),
            scroll: UniformListScrollHandle::new(),
            running: None,
            history,
            style: (theme.name, code_font_name(), code_font_size().to_bits()),
            _subs: subs,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |e, cx| e.focus(window, cx));
    }

    /// The caret is at the very end of the input.
    fn caret_at_end(&self, text: &str, cx: &App) -> bool {
        let pos = self.input.read(cx).cursor();
        pos.line == text.split('\n').count() - 1
            && pos.col == text.rsplit('\n').next().map_or(0, str::len)
    }

    fn on_editor(&mut self, ev: &EditorEvent, cx: &mut Context<Self>) {
        match ev {
            EditorEvent::Enter => self.enter(false, cx),
            EditorEvent::Submit => self.enter(true, cx),
            EditorEvent::HistoryPrev => {
                let current = self.input.read(cx).text();
                let entry = self.history.borrow_mut().prev(&current).map(str::to_owned);
                if let Some(text) = entry {
                    self.input.update(cx, |e, cx| e.set_text_at_end(&text, cx));
                }
            }
            EditorEvent::HistoryNext => {
                let entry = self.history.borrow_mut().next().map(str::to_owned);
                if let Some(text) = entry {
                    self.input.update(cx, |e, cx| e.set_text_at_end(&text, cx));
                }
            }
            EditorEvent::Changed | EditorEvent::CursorMoved => cx.notify(),
        }
    }

    /// Enter (`force` false) or Ctrl+Enter: run the input, or continue it on a
    /// new line when the statement is not finished.
    fn enter(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let text = self.input.read(cx).text();
        if text.trim().is_empty() {
            return;
        }
        // Enter inside the text is an ordinary line break.
        if !force && !self.caret_at_end(&text, cx) {
            self.input.update(cx, |e, cx| e.newline(cx));
            return;
        }
        self.execute(text, force, cx);
    }

    /// Runs `source` in the host, like typing it at IDA's console.
    pub fn execute(&mut self, source: String, force: bool, cx: &mut Context<Self>) {
        if !self.session.read(cx).is_ready() {
            self.push_text(Kind::Err, "No database is open.\n");
            cx.notify();
            return;
        }
        let ea = self.session.read(cx).cursor;
        self.running = Some(Running {
            started: Instant::now(),
            file: false,
        });
        self.session.update(cx, |s, cx| s.python_started(cx));
        let fut = self.session.read(cx).call(PyEval {
            source: source.clone(),
            ea,
            force,
        });
        self.tick(cx);
        cx.spawn(async move |this, cx| {
            let reply = fut.await;
            this.update(cx, |c, cx| {
                c.finished(Some(source), reply.map_err(|e| e.to_string()), cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// File > Script file: runs a Python file.
    pub fn run_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.running.is_some() {
            self.push_text(Kind::Err, "A script is already running.\n");
            cx.notify();
            return;
        }
        if !self.session.read(cx).is_ready() {
            self.push_text(Kind::Err, "No database is open.\n");
            cx.notify();
            return;
        }
        self.push_text(Kind::Note, &format!("Running script {}\n", path.display()));
        let ea = self.session.read(cx).cursor;
        self.running = Some(Running {
            started: Instant::now(),
            file: true,
        });
        self.session.update(cx, |s, cx| s.python_started(cx));
        let fut = self.session.read(cx).call(PyRunFile {
            path: path.display().to_string(),
            ea,
        });
        self.tick(cx);
        cx.spawn(async move |this, cx| {
            let reply = fut.await;
            this.update(cx, |c, cx| {
                c.finished(None, reply.map_err(|e| e.to_string()), cx)
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Keeps the elapsed time on screen while something runs.
    fn tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let running = this
                    .update(cx, |c, cx| {
                        if c.running.is_some() {
                            cx.notify();
                        }
                        c.running.is_some()
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        })
        .detach();
    }

    fn finished(
        &mut self,
        source: Option<String>,
        reply: Result<PyReply, String>,
        cx: &mut Context<Self>,
    ) {
        self.running = None;
        match reply {
            Ok(r) if r.incomplete => {
                self.session
                    .update(cx, |s, cx| s.python_finished(false, cx));
                // The statement is not finished: continue on the next line.
                self.input.update(cx, |e, cx| e.newline(cx));
            }
            Ok(r) => {
                self.session
                    .update(cx, |s, cx| s.python_finished(r.changed, cx));
                if let Some(src) = &source {
                    self.echo(src);
                    self.remember(src, cx);
                    self.input.update(cx, |e, cx| e.set_text("", cx));
                }
                for chunk in &r.chunks {
                    let kind = match chunk.0.as_str() {
                        "err" => Kind::Err,
                        "result" => Kind::Result,
                        "tb" => Kind::Traceback,
                        _ => Kind::Out,
                    };
                    self.push_text(kind, &chunk.1);
                }
                if r.elapsed_ms >= 1000 {
                    let how = if r.error.is_some() {
                        "stopped"
                    } else {
                        "finished"
                    };
                    self.push_text(
                        Kind::Note,
                        &format!("Script {how} after {:.1} s\n", r.elapsed_ms as f32 / 1000.),
                    );
                }
            }
            Err(e) => {
                self.session
                    .update(cx, |s, cx| s.python_finished(false, cx));
                if let Some(src) = &source {
                    self.echo(src);
                    self.input.update(cx, |e, cx| e.set_text("", cx));
                }
                self.push_text(Kind::Err, &format!("{e}\n"));
            }
        }
        cx.notify();
    }

    fn remember(&mut self, source: &str, cx: &mut Context<Self>) {
        let cap = settings::get(cx).python_history_size;
        let mut h = self.history.borrow_mut();
        h.set_cap(cap);
        h.push(source);
        if let Err(e) = h.save(&history_path()) {
            eprintln!("python history: cannot save: {e}");
        }
    }

    /// The submitted source above its output, with `>>>` / `...` prompts.
    fn echo(&mut self, source: &str) {
        let lines = Rc::make_mut(&mut self.lines);
        for (i, l) in source.trim_end().split('\n').enumerate() {
            lines.push(Line {
                kind: Kind::Input(i == 0),
                text: l.replace('\t', "    "),
                open: false,
            });
        }
        self.trim();
        self.scroll_to_end();
    }

    /// Appends `text` to the output; a chunk that ends without a newline keeps
    /// its line open for the next chunk of the same kind.
    fn push_text(&mut self, kind: Kind, text: &str) {
        let lines = Rc::make_mut(&mut self.lines);
        for piece in text.split_inclusive('\n') {
            let (body, closed) = match piece.strip_suffix('\n') {
                Some(b) => (b.trim_end_matches('\r'), true),
                None => (piece, false),
            };
            let body = body.replace('\t', "    ");
            match lines.last_mut() {
                Some(last) if last.open && last.kind == kind => {
                    last.text.push_str(&body);
                    last.open = !closed;
                }
                _ => lines.push(Line {
                    kind,
                    text: body,
                    open: !closed,
                }),
            }
        }
        self.trim();
        self.scroll_to_end();
    }

    fn trim(&mut self) {
        if self.lines.len() > MAX_LINES {
            let extra = self.lines.len() - MAX_LINES;
            Rc::make_mut(&mut self.lines).drain(..extra);
        }
    }

    fn scroll_to_end(&mut self) {
        if !self.lines.is_empty() {
            self.scroll
                .scroll_to_item(self.lines.len() - 1, ScrollStrategy::Bottom);
        }
    }

    pub fn clear_output(&mut self, cx: &mut Context<Self>) {
        self.lines = Rc::default();
        cx.notify();
    }

    /// Asks the host to raise KeyboardInterrupt in the running script.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.running.is_none() {
            return;
        }
        self.push_text(Kind::Note, "Interrupt sent\n");
        let fut = self.session.read(cx).call(Interrupt {});
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |c, cx| {
                    c.push_text(Kind::Err, &format!("interrupt: {e}\n"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// Keeps the input in step with the app theme and code font.
    fn sync_style(&mut self, cx: &mut Context<Self>) {
        let theme = Theme::get(cx);
        let now = (theme.name, code_font_name(), code_font_size().to_bits());
        if now != self.style {
            self.style = now;
            self.input.update(cx, |e, cx| {
                e.set_theme(theme.editor(), cx);
                e.set_font(
                    code_font_name(),
                    px(code_font_size()),
                    px(code_line_height()),
                    cx,
                );
            });
        }
    }
}

fn elapsed_text(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

impl Render for PyConsole {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_style(cx);
        let t = Theme::get(cx);
        let lh = code_line_height();
        let lines = self.lines.clone();
        let list = uniform_list("py-output", lines.len(), move |range, _window, _cx| {
            range
                .map(|ix| {
                    let l = &lines[ix];
                    let (gutter, color) = match l.kind {
                        Kind::Input(first) => (if first { ">>>" } else { "..." }, t.muted),
                        Kind::Out => ("", t.text),
                        Kind::Err => ("", t.warning),
                        Kind::Result => ("", t.syntax.code_name),
                        Kind::Traceback => ("", t.danger),
                        Kind::Note => ("", t.faint),
                    };
                    div()
                        .h(px(lh))
                        .flex_none()
                        .flex()
                        .items_center()
                        .px(px(12.))
                        .font_family(code_font_name())
                        .text_size(px(code_font_size()))
                        .line_height(px(lh))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(
                            div()
                                .w(px(36.))
                                .flex_none()
                                .text_color(hsla(if matches!(l.kind, Kind::Input(true)) {
                                    t.accent
                                } else {
                                    t.faint
                                }))
                                .child(gutter),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_color(hsla(color))
                                .when(l.kind == Kind::Note, |d| d.italic())
                                .child(l.text.clone()),
                        )
                })
                .collect()
        })
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        let bar = scrollbar::scrollbar(
            "py-scroll",
            Source::list(&self.scroll),
            Axes::V,
            &t,
            window,
            cx,
        );
        let rows = self.input.read(cx).line_count().clamp(1, MAX_INPUT_ROWS);
        let running = self.running.as_ref().map(|r| (r.started.elapsed(), r.file));
        let ready = matches!(self.session.read(cx).phase, Phase::Ready);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(t.surface))
            .child(
                div().relative().flex_1().min_h_0().flex().flex_col().child(list).child(bar).when(self.lines.is_empty(), |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(hsla(t.faint))
                            .child(if ready {
                                "IDAPython. Enter runs, Shift+Enter adds a line, Ctrl+Enter forces a run."
                            } else {
                                "Open a database to use the console."
                            }),
                    )
                }),
            )
            .when_some(running, |d, (elapsed, file)| {
                d.child(
                    div()
                        .h(px(28.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .bg(hsla(t.surface2))
                        .border_t_1()
                        .border_color(hsla(t.border))
                        .text_size(px(12.))
                        .child(spinner("py-run-spin", 14., hsla(t.accent)))
                        .child(div().text_color(hsla(t.text)).child(if file { "Running script" } else { "Running" }))
                        .child(ui::mono_note(elapsed_text(elapsed), &t))
                        .child(div().flex_1())
                        .child(div().text_color(hsla(t.muted)).child("The database is busy until it returns"))
                        .child(ui::button("py-cancel", "Cancel", ButtonKind::Danger, true, &t).on_click(cx.listener(|c, _, _, cx| c.cancel(cx)))),
                )
            })
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_start()
                    .border_t_1()
                    .border_color(hsla(t.border))
                    .bg(hsla(t.surface))
                    .child(
                        div()
                            .w(px(48.))
                            .flex_none()
                            .pl(px(12.))
                            .pt(px(4.))
                            .h(px(lh))
                            .font_family(code_font_name())
                            .text_size(px(code_font_size()))
                            .line_height(px(lh))
                            .text_color(hsla(if self.running.is_some() { t.faint } else { t.accent }))
                            .child(">>>"),
                    )
                    .child(div().flex_1().min_w_0().h(px(lh * rows as f32 + 8.)).py(px(4.)).child(self.input.clone())),
            )
    }
}
