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
use crate::session::{Phase, Session, SessionEvent};
use crate::settings;
use crate::theme::{Theme, code_font_name, code_font_size, code_line_height, hsla};
use crate::ui::{self, ButtonKind, spinner};
use gpui::{
    App, Context, Entity, Global, IntoElement, Render, ScrollStrategy, SharedString, Subscription,
    UniformListScrollHandle, Window, div, prelude::*, px, uniform_list,
};
use ida_proto::{Interrupt, PyComplete, PyEval, PyReply, PyRunFile};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};
use zui_editor::{Editor, EditorEvent, Pos, PythonHighlighter};

/// Output kept in the list; older lines fall off the top.
const MAX_LINES: usize = 20_000;
/// The input grows with its text up to this many lines, then scrolls.
const MAX_INPUT_ROWS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// An echoed input line; the flag marks the first line of a command.
    Input(bool),
    /// An echoed IDC input line.
    IdcInput(bool),
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

/// How long after Cancel a script may keep running before "Kill host" shows.
const KILL_OFFER_AFTER: Duration = Duration::from_secs(4);

struct Running {
    started: Instant,
    /// When Cancel was pressed; a script that ignores it for a while (it is
    /// inside a long C call) gets a "Kill host" button.
    cancelled: Option<Instant>,
    /// The first click on "Kill host" asks again before killing.
    kill_armed: bool,
    /// An IDC statement: its messages stream in as IDA's log lines, and the
    /// reply must not replace them.
    idc: bool,
    /// The input, until it has been echoed: it is echoed with the first
    /// streamed output (a statement that turns out incomplete never is).
    echo: Option<String>,
    /// Where streamed output starts, so those lines can be replaced by the
    /// typed output the reply carries.
    out_mark: Option<usize>,
    /// A script file rather than console input.
    file: bool,
}

/// Completion candidates on offer for the name that starts at `start`.
struct Completions {
    items: Vec<String>,
    /// The candidate Tab last inserted, while cycling.
    sel: Option<usize>,
    start: Pos,
}

pub struct PyConsole {
    session: Entity<Session>,
    completions: Option<Completions>,
    /// The input text right after the completion edited it: that change does
    /// not close the candidate list.
    completed_text: Option<String>,
    input: Entity<Editor>,
    lines: Rc<Vec<Line>>,
    scroll: UniformListScrollHandle,
    running: Option<Running>,
    /// The input is IDC (IDA's own script language) rather than Python.
    pub idc: bool,
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
            cx.subscribe(&session, |this, _, ev: &SessionEvent, cx| {
                // Output as it is produced; the reply replaces it with the
                // same text split by stream.
                if let SessionEvent::HostLog(lines) = ev
                    && this.running.is_some()
                {
                    if let Some(src) = this.running.as_mut().and_then(|r| r.echo.take()) {
                        this.echo(&src);
                    }
                    let at = this.lines.len();
                    if let Some(r) = this.running.as_mut() {
                        r.out_mark.get_or_insert(at);
                    }
                    for l in lines {
                        this.push_text(
                            Kind::Out,
                            &format!(
                                "{l}
"
                            ),
                        );
                    }
                    cx.notify();
                }
            }),
        ];
        let history = history(cx);
        Self {
            session,
            completions: None,
            completed_text: None,
            input,
            lines: Rc::default(),
            scroll: UniformListScrollHandle::new(),
            running: None,
            idc: false,
            history,
            style: (theme.name, code_font_name(), code_font_size().to_bits()),
            _subs: subs,
        }
    }

    /// Switches the console between Python and IDC.
    pub fn toggle_language(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        self.idc = !self.idc;
        self.completions = None;
        let note = if self.idc {
            "IDC: an expression, or statements ending in ;  (Message() output shows as IDA's log)"
        } else {
            "Python"
        };
        self.push_text(
            Kind::Note,
            &format!(
                "{note}
"
            ),
        );
        cx.notify();
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
            EditorEvent::Complete => self.complete(cx),
            EditorEvent::Changed => {
                let text = self.input.read(cx).text();
                if self.completed_text.as_deref() != Some(text.as_str()) {
                    self.completions = None;
                }
                cx.notify();
            }
            EditorEvent::CursorMoved => cx.notify(),
        }
    }

    /// Tab after a name: offer completions, or step through the ones on offer.
    fn complete(&mut self, cx: &mut Context<Self>) {
        if self.running.is_some() || !self.session.read(cx).is_ready() {
            return;
        }
        if let Some(c) = self.completions.as_mut()
            && c.items.len() > 1
        {
            let next = c.sel.map_or(0, |i| (i + 1) % c.items.len());
            c.sel = Some(next);
            let (item, start) = (c.items[next].clone(), c.start);
            self.replace_word(start, &item, cx);
            return;
        }
        let text = self.input.read(cx).text();
        let pos = self.input.read(cx).cursor();
        let line = text.split('\n').nth(pos.line).unwrap_or("");
        let word = completion_word(&line[..pos.col.min(line.len())]).to_owned();
        if word.is_empty() {
            self.input
                .update(cx, |e, cx| e.insert_at_cursor("    ", cx));
            return;
        }
        let start = Pos {
            line: pos.line,
            col: pos.col - word.len(),
        };
        let fut = self
            .session
            .read(cx)
            .call(PyComplete { word: word.clone() });
        cx.spawn(async move |this, cx| {
            let items = fut.await.unwrap_or_default();
            this.update(cx, |c, cx| c.offer(word, start, items, cx))
                .ok();
        })
        .detach();
    }

    /// The candidates for `word` arrived: insert what is certain, list the rest.
    fn offer(&mut self, word: String, start: Pos, items: Vec<String>, cx: &mut Context<Self>) {
        // The caret moved on while the host was thinking.
        let pos = self.input.read(cx).cursor();
        if pos.line != start.line || pos.col != start.col + word.len() || items.is_empty() {
            return;
        }
        let prefix = common_prefix(&items);
        if prefix.len() > word.len() {
            self.replace_word(start, &prefix, cx);
        }
        self.completions = (items.len() > 1).then_some(Completions {
            items,
            sel: None,
            start,
        });
        cx.notify();
    }

    /// Replaces the text from `start` to the caret with `text`.
    fn replace_word(&mut self, start: Pos, text: &str, cx: &mut Context<Self>) {
        self.input.update(cx, |e, cx| {
            let head = e.cursor();
            e.set_selection(start, head, cx);
            e.insert_at_cursor(text, cx);
        });
        self.completed_text = Some(self.input.read(cx).text());
        cx.notify();
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
            cancelled: None,
            kill_armed: false,
            idc: self.idc,
            echo: Some(source.clone()),
            out_mark: None,
            file: false,
        });
        self.session.update(cx, |s, cx| s.python_started(cx));
        let (wire, force) = if self.idc {
            (idc_source(&source), true)
        } else {
            (source.clone(), force)
        };
        let fut = self.session.read(cx).call(PyEval {
            source: wire,
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
            cancelled: None,
            kill_armed: false,
            idc: false,
            echo: None,
            out_mark: None,
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
        let run = self.running.take();
        // Streamed lines give way to the reply's typed output.
        if let Some(mark) = run.as_ref().filter(|r| !r.idc).and_then(|r| r.out_mark) {
            let keep = mark.min(self.lines.len());
            Rc::make_mut(&mut self.lines).truncate(keep);
        }
        let echoed = run.as_ref().is_some_and(|r| r.echo.is_none());
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
                    if !echoed {
                        self.echo(src);
                    }
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
                kind: if self.idc {
                    Kind::IdcInput(i == 0)
                } else {
                    Kind::Input(i == 0)
                },
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

    /// The whole transcript as text (the output context menu's Copy).
    pub fn output_text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.text.trim_end_matches('\n'))
            .collect::<Vec<_>>()
            .join("\n")
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
        if let Some(r) = self.running.as_mut() {
            r.cancelled.get_or_insert_with(Instant::now);
        }
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

    /// "Kill host": the script ignored Cancel. The first click asks again
    /// (unsaved changes are lost), the second ends the host process.
    fn kill_host(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.running.as_mut() else {
            return;
        };
        if !r.kill_armed {
            r.kill_armed = true;
            cx.notify();
            return;
        }
        self.push_text(Kind::Err, "Host killed.\n");
        self.session.update(cx, |s, cx| s.kill_host(cx));
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
                        Kind::IdcInput(first) => (if first { "idc" } else { "..." }, t.muted),
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
                                .text_color(hsla(
                                    if matches!(l.kind, Kind::Input(true) | Kind::IdcInput(true)) {
                                        t.accent
                                    } else {
                                        t.faint
                                    },
                                ))
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
        let running = self.running.as_ref().map(|r| {
            let stuck = r.cancelled.is_some_and(|c| c.elapsed() > KILL_OFFER_AFTER);
            (r.started.elapsed(), r.file, stuck, r.kill_armed)
        });
        let ready = matches!(self.session.read(cx).phase, Phase::Ready);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(t.surface))
            .child(
                div().relative().flex_1().min_h_0().flex().flex_col().id("py-output-area").on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(|v, ev: &gpui::MouseDownEvent, _, cx| {
                        let text = v.output_text();
                        let me = cx.entity();
                        let entries = vec![
                            crate::menubar::run_entry("Copy output", "", move |_, cx| {
                                zui_editor::clipboard::write(text.clone(), cx)
                            }),
                            crate::menubar::run_entry("Clear output", "", move |_, cx| {
                                me.update(cx, |c, cx| c.clear_output(cx))
                            }),
                        ];
                        crate::context_menu::show(&v.session, ev.position, "Python console", entries, false, cx);
                    }),
                ).child(list).child(bar).when_some(self.completions.as_ref(), |d, c| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(48.))
                            .right(px(16.))
                            .bottom(px(4.))
                            .max_h(px(lh * 4. + 8.))
                            .overflow_hidden()
                            .flex()
                            .flex_wrap()
                            .gap_x(px(12.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded(px(6.))
                            .bg(hsla(t.surface2))
                            .border_1()
                            .border_color(hsla(t.border_strong))
                            .font_family(code_font_name())
                            .text_size(px(code_font_size()))
                            .line_height(px(lh))
                            .children(c.items.iter().enumerate().map(|(i, item)| {
                                div()
                                    .px(px(4.))
                                    .rounded(px(3.))
                                    .when(c.sel == Some(i), |d| d.bg(hsla(t.accent)).text_color(hsla(t.bg)))
                                    .child(item.clone())
                            })),
                    )
                }).when(self.lines.is_empty() && self.completions.is_none(), |d| {
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
                                if self.idc {
                                    "IDC. Enter runs, Shift+Enter adds a line. Switch back to Python with the language button."
                                } else {
                                    "IDAPython. Enter runs, Shift+Enter adds a line, Ctrl+Enter forces a run."
                                }
                            } else {
                                "Open a database to use the console."
                            }),
                    )
                }),
            )
            .when_some(running, |d, (elapsed, file, stuck, armed)| {
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
                        .child(div().text_color(hsla(t.muted)).child(if armed {
                            "Unsaved changes in this database will be lost"
                        } else if stuck {
                            "Cancel only works between Python steps; this call has not returned"
                        } else {
                            "The database is busy until it returns"
                        }))
                        .when(stuck, |d| {
                            d.child(
                                ui::button("py-kill", if armed { "Kill host now" } else { "Kill host…" }, ButtonKind::Danger, true, &t)
                                    .on_click(cx.listener(|c, _, _, cx| c.kill_host(cx))),
                            )
                        })
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
                            .child(if self.idc { "idc" } else { ">>>" }),
                    )
                    .child(div().flex_1().min_w_0().h(px(lh * rows as f32 + 8.)).py(px(4.)).child(self.input.clone())),
            )
    }
}

/// The dotted name ending at the caret, which is what Python's completer
/// completes: `idaapi.get_f` in `x = idaapi.get_f`. Empty when the caret is
/// not after a name (after `(`, a space, a digit run or a bare `.`).
fn completion_word(prefix: &str) -> &str {
    let start = prefix
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_alphanumeric() || c == '_' || c == '.')
        .last()
        .map_or(prefix.len(), |(i, _)| i);
    let word = &prefix[start..];
    match word.chars().next() {
        Some(c) if c.is_alphabetic() || c == '_' => word,
        _ => "",
    }
}

/// The longest text all `items` start with.
fn common_prefix(items: &[String]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut len = first.len();
    for s in &items[1..] {
        len = first
            .bytes()
            .zip(s.bytes())
            .take(len)
            .take_while(|(a, b)| a == b)
            .count();
    }
    while !first.is_char_boundary(len) {
        len -= 1;
    }
    first[..len].to_owned()
}

/// The Python that runs `source` as IDC through IDA's own evaluator: first as
/// an expression (the value is printed), else as statements.
fn idc_source(source: &str) -> String {
    let literal = serde_json::to_string(source).unwrap_or_else(|_| "\"\"".to_owned());
    format!("{IDC_RUNNER}\n_izs_idc({literal})\n")
}

const IDC_RUNNER: &str = r#"import ida_expr as _e, ida_idaapi as _i, sys as _s
class _R:
    def __init__(self, t): self.t = t
    def __repr__(self): return self.t
def _izs_idc(src):
    def err(r):
        if isinstance(r, tuple):
            return "" if r[0] else (r[1] or "error")
        if isinstance(r, bool):
            return "" if r else "syntax error"
        return r or ""
    def fmt(rv):
        t = rv.vtype
        if t == _e.VT_STR:
            return '"' + rv.c_str() + '"'
        if t == _e.VT_FLOAT:
            return repr(rv.e)
        n = rv.i64 if t == _e.VT_INT64 else rv.num
        return "%d (0x%X)" % (n, n & 0xFFFFFFFFFFFFFFFF)
    rv = _e.idc_value_t()
    e = err(_e.eval_idc_expr(rv, _i.BADADDR, src))
    if not e:
        return _R(fmt(rv))
    c = _e.compile_idc_snippet("_izs_snip", src)
    c = "syntax error" if c is None else err(c)
    if c:
        plain = ";" not in src and "{" not in src
        _s.stderr.write("IDC: " + (e if plain else c) + "\n")
        return None
    rv = _e.idc_value_t()
    r = err(_e.eval_idc_expr(rv, _i.BADADDR, "_izs_snip()"))
    if r:
        _s.stderr.write("IDC: " + r + "\n")
    return None
"#;

#[cfg(test)]
mod tests {
    use super::{common_prefix, completion_word};

    #[test]
    fn the_word_is_the_dotted_name_at_the_caret() {
        assert_eq!(completion_word("idaapi.get_f"), "idaapi.get_f");
        assert_eq!(completion_word("x = foo(bar"), "bar");
        assert_eq!(completion_word("a.b.c."), "a.b.c.");
        assert_eq!(completion_word("_priv"), "_priv");
    }

    #[test]
    fn nothing_to_complete_after_punctuation_space_or_digits() {
        assert_eq!(completion_word(""), "");
        assert_eq!(completion_word("print("), "");
        assert_eq!(completion_word("x = "), "");
        assert_eq!(completion_word("1.5"), "");
        assert_eq!(completion_word("x[0].na"), "");
    }

    #[test]
    fn common_prefix_stops_at_the_first_difference() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            common_prefix(&v(&["get_func", "get_func_qty", "get_fchunk"])),
            "get_f"
        );
        assert_eq!(common_prefix(&v(&["only"])), "only");
        assert_eq!(common_prefix(&v(&["ab", "cd"])), "");
        assert_eq!(common_prefix(&[]), "");
        assert_eq!(common_prefix(&v(&["née", "nèe"])), "n");
    }
}
