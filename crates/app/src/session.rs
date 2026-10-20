// One open database: the host process, navigation state and change tracking.
// Views subscribe to `SessionEvent`s; they never talk to each other directly.
use crate::debugger::DebugModel;
use crate::open_dialog::{Loader, OpenRequest, PROCESSORS};
use crate::settings;
use crate::status::{STATUS_TTL, StatusLine};
use futures::StreamExt;
use gpui::{App, Context, EventEmitter, SharedString, Task};
use ida_proto::client::HostConfig;
use ida_proto::{
    AnalysisEvent, ChangeOrigin, Client, ClientError, DbChanged, DbInfo, Ea, HostEvent, Method,
};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(crate) mod debug;

const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Listing,
    Graph,
    Pseudocode,
    Hex,
}

/// Where the caret sits inside the focused code view, for the status bar,
/// the inspector and the live xrefs panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Caret {
    pub origin: Option<Origin>,
    /// 1-based line within the view (pseudocode) or block (graph); the
    /// listing has no stable line numbers.
    pub line: Option<u32>,
    /// 1-based column.
    pub col: u32,
    /// The graph block the caret is in.
    pub block: Option<usize>,
    /// The name or identifier under the caret.
    pub token: Option<String>,
    /// The 0-based operand of the instruction under the caret (listing and
    /// graph), for Edit > Operand.
    pub operand: Option<u32>,
}

/// A view asks for a context menu (see `context_menu.rs`).
pub struct ContextMenuRequest {
    pub pos: gpui::Point<gpui::Pixels>,
    pub title: SharedString,
    pub entries: Vec<crate::menubar::Entry>,
    /// Opened from the keyboard: highlight the first entry.
    pub keyboard: bool,
}

/// A view asks for the comment editor (`commands::comment_ea`).
pub struct CommentRequest {
    pub ea: Ea,
    pub repeatable: bool,
    pub text: String,
}

pub enum SessionEvent {
    ContextMenu(ContextMenuRequest),
    EditComment(CommentRequest),
    /// The marked positions were (re)loaded.
    MarksChanged,
    /// The caret moved within a view (or onto another token).
    CaretChanged,
    Opened,
    /// A jump the user asked for; views should bring `ea` into view.
    Navigated {
        ea: Ea,
    },
    /// The caret moved inside a view; others may follow lazily.
    CursorMoved {
        ea: Ea,
        origin: Origin,
    },
    /// The database changed; views refetch what they show.
    Changed,
    Prompt(PromptRequest),
    ShowXrefs {
        ea: Ea,
        to: bool,
    },
    /// A ready-made list of references, e.g. to a structure member.
    ShowXrefList {
        title: String,
        rows: Vec<ida_proto::XrefRow>,
    },
    /// Open an editor tab (local types, a structure, an enum, a stack frame).
    OpenDoc(crate::docs::DocKey),
    ListsReloaded,
    /// The debugger state changed (process state, breakpoints, registers).
    DebugChanged,
    /// The process stopped at `ip`.
    DebugStopped,
    /// The database moved to match the process (or back); every address
    /// the view holds is stale.
    Rebased {
        delta: i64,
    },
    /// IDA printed these lines (the host sends them while a request still runs).
    HostLog(Vec<String>),
    Closed,
}

pub type SubmitFn = Rc<dyn Fn(String, &mut App)>;

#[derive(Clone)]
pub struct PromptRequest {
    pub title: SharedString,
    pub placeholder: SharedString,
    pub initial: String,
    pub on_submit: SubmitFn,
    /// Label of the confirm button.
    pub confirm: SharedString,
    /// The text is a name: check it while typing.
    pub name: bool,
}

pub enum Phase {
    Empty,
    Loading { path: PathBuf, started: Instant },
    Ready,
    Failed(SharedString),
}

pub struct Session {
    client: Option<Client>,
    pub phase: Phase,
    pub info: Option<DbInfo>,
    pub cursor: Ea,
    pub caret: Caret,
    history: Vec<Ea>,
    hist_pos: usize,
    pub dirty: bool,
    /// What the open (or loading) database was opened with.
    pub request: Option<OpenRequest>,
    pub revision: u64,
    pub log: Vec<SharedString>,
    pub status: StatusLine,
    /// How each operand of the item at the cursor is shown (check marks of
    /// Edit > Operand); refreshed by `Focus` after the caret settles.
    pub op_repr: Vec<ida_proto::OpReprRow>,
    /// The marked positions (IDA's bookmarks), for the listing gutter.
    pub marks: Vec<ida_proto::MarkRow>,
    /// Function and name lists, shared by the side panel and jump-anywhere.
    pub functions: Rc<Vec<ida_proto::FuncRow>>,
    pub names: Rc<Vec<ida_proto::NameRow>>,
    /// The auto-analysis run in progress (opening, or after an edit), with
    /// the latest report the host sent.
    pub analysis: Option<AnalysisEvent>,
    /// A script is executing; the host cannot answer other requests until it
    /// returns, so refreshes wait.
    pub python_running: bool,
    /// The debugger: process state, breakpoints, registers, stack.
    pub dbg: DebugModel,
    /// Changes the UI did not make itself (a script) wait here for a refresh.
    ext_pending: bool,
    ext_lists: bool,
    ext_task: Option<Task<()>>,
    _log_pump: Option<Task<()>>,
    _event_pump: Option<Task<()>>,
}

/// Script changes are folded over this long before the views refresh.
const EXTERNAL_REFRESH_DELAY: Duration = Duration::from_millis(150);

impl EventEmitter<SessionEvent> for Session {}

impl Session {
    pub fn new() -> Self {
        Self {
            client: None,
            phase: Phase::Empty,
            info: None,
            cursor: 0,
            caret: Caret::default(),
            history: Vec::new(),
            hist_pos: 0,
            dirty: false,
            request: None,
            revision: 0,
            log: Vec::new(),
            status: StatusLine::default(),
            op_repr: Vec::new(),
            marks: Vec::new(),
            functions: Rc::default(),
            names: Rc::default(),
            analysis: None,
            python_running: false,
            dbg: DebugModel::new(),
            ext_pending: false,
            ext_lists: false,
            ext_task: None,
            _log_pump: None,
            _event_pump: None,
        }
    }

    /// Reacts to an unsolicited host event. The UI's own edits (origin
    /// `request`) are refreshed by the request that made them; only changes
    /// nobody asked the UI to refresh, i.e. scripts, are handled here.
    fn on_host_event(&mut self, ev: HostEvent, cx: &mut Context<Self>) {
        match ev {
            // The Output panel reads the log channel; the Python console wants
            // the lines as they come while a script runs.
            HostEvent::Log { lines } => cx.emit(SessionEvent::HostLog(lines)),
            HostEvent::Analysis(a) => {
                self.analysis = (!a.done).then_some(a);
                cx.notify();
            }
            HostEvent::DbChanged(DbChanged {
                origin: ChangeOrigin::Request,
                ..
            }) => {}
            HostEvent::DbChanged(d) => self.mark_external(d.affects_lists(), cx),
            HostEvent::Debug(d) => self.on_debug_event(d, cx),
        }
    }

    /// Something changed behind the UI's back: refresh once the changes
    /// stop coming and the host is free to answer.
    fn mark_external(&mut self, lists: bool, cx: &mut Context<Self>) {
        self.ext_pending = true;
        self.ext_lists |= lists;
        if self.ext_task.is_some() {
            return;
        }
        self.ext_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(EXTERNAL_REFRESH_DELAY).await;
            this.update(cx, |s, cx| {
                s.ext_task = None;
                // The script's end schedules the refresh again.
                if s.python_running || !s.ext_pending {
                    return;
                }
                let lists = std::mem::take(&mut s.ext_lists);
                s.ext_pending = false;
                s.dirty = true;
                s.revision += 1;
                if lists {
                    s.reload_lists(cx);
                }
                cx.emit(SessionEvent::Changed);
                cx.notify();
            })
            .ok();
        }));
    }

    /// A script started. Views are not refreshed while it runs.
    pub fn python_started(&mut self, cx: &mut Context<Self>) {
        self.python_running = true;
        cx.notify();
    }

    /// A script finished; `changed` is what the host reported. The events
    /// normally got here first, so this only makes sure nothing is missed.
    pub fn python_finished(&mut self, changed: bool, cx: &mut Context<Self>) {
        self.python_running = false;
        if changed {
            self.mark_external(true, cx);
        } else if self.ext_pending {
            self.mark_external(self.ext_lists, cx);
        }
        cx.notify();
    }

    /// Refetches the function and name lists.
    pub fn reload_lists(&mut self, cx: &mut Context<Self>) {
        let funcs = self.call(ida_proto::Functions {});
        let names = self.call(ida_proto::Names {});
        cx.spawn(async move |this, cx| {
            let funcs = funcs.await;
            let names = names.await;
            this.update(cx, |s, cx| {
                if let Ok(f) = funcs {
                    s.functions = Rc::new(f);
                }
                if let Ok(n) = names {
                    s.names = Rc::new(n);
                }
                cx.emit(SessionEvent::ListsReloaded);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Output panel > Clear.
    pub fn clear_log(&mut self, cx: &mut Context<Self>) {
        self.log.clear();
        cx.notify();
    }

    /// Re-reads the marked positions.
    pub fn reload_marks(&mut self, cx: &mut Context<Self>) {
        let fut = self.call(ida_proto::MarkList {});
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = fut.await {
                this.update(cx, |s, cx| {
                    s.marks = rows;
                    cx.emit(SessionEvent::MarksChanged);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// The function containing `ea`, from the cached list.
    pub fn function_at(&self, ea: Ea) -> Option<&ida_proto::FuncRow> {
        let i = self.functions.partition_point(|f| f.start <= ea);
        self.functions.get(i.checked_sub(1)?).filter(|f| ea < f.end)
    }

    /// True when no host is running, so there is nothing to close.
    pub fn client_is_none(&self) -> bool {
        self.client.is_none()
    }

    /// An auto-analysis is running: the initial one while the database opens,
    /// or one an edit started.
    pub fn analyzing(&self) -> bool {
        self.client.is_some()
            && (matches!(self.phase, Phase::Loading { .. }) || self.analysis.is_some())
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready)
    }

    /// Starts a host and opens `req.path` (a binary or an existing .i64) with
    /// the requested loader, processor and base address. Each session owns
    /// one host: the caller closes an open session first (or uses a new one).
    pub fn open_with(&mut self, req: OpenRequest, cx: &mut Context<Self>) {
        // A host left over from an open that failed.
        if let Some(c) = self.client.take() {
            c.kill();
        }
        // IDA names the database `<input>.i64`; one that has to be built from
        // scratch is unsaved work.
        let is_db = req
            .path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("i64") || e.eq_ignore_ascii_case("idb"));
        let fresh = !is_db && !req.database_path().exists();
        let path = req.path.clone();
        self.request = Some(req.clone());
        self.info = None;
        self.dirty = false;
        self.history.clear();
        self.hist_pos = 0;
        self.log.clear();
        self.status.clear();
        self.phase = Phase::Loading {
            path: path.clone(),
            started: Instant::now(),
        };
        cx.notify();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            // A forced processor means opening a copy under a private
            // extension; copying a big blob must not block the UI.
            let prepared = executor.spawn({
                let req = req.clone();
                async move { prepare_input(&req) }
            });
            let prepared = prepared.await;
            this.update(cx, |s, cx| match prepared {
                Ok(()) => s.start_host(req, fresh, cx),
                Err(e) => {
                    s.phase = Phase::Failed(e.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn start_host(&mut self, req: OpenRequest, fresh: bool, cx: &mut Context<Self>) {
        if let Some(problem) = host_config(&req).problem() {
            self.phase = Phase::Failed(problem.message().into());
            cx.notify();
            return;
        }
        let (client, mut logs, mut events) = match Client::spawn_with_events(&host_config(&req)) {
            Ok(t) => t,
            Err(e) => {
                self.phase = Phase::Failed(e.to_string().into());
                cx.notify();
                return;
            }
        };
        self.analysis = None;
        self._event_pump = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update(cx, |s, cx| s.on_host_event(ev, cx)).is_err() {
                    break;
                }
            }
        }));
        self._log_pump = Some(cx.spawn(async move |this, cx| {
            while let Some(line) = logs.next().await {
                if this
                    .update(cx, |s, cx| {
                        s.log.push(line.into());
                        if s.log.len() > 5000 {
                            s.log.drain(..1000);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        let open = client.call(ida_proto::Open {
            path: req.input_path().display().to_string(),
            auto_analyze: req.run_auto,
            base: req.base,
        });
        self.client = Some(client);
        let (indent, cmt_indent) = {
            let st = settings::get(cx);
            (st.listing_indent, st.listing_comment)
        };
        cx.spawn(async move |this, cx| {
            let result = open.await;
            // The database was closed (the analysis was stopped) while the
            // open was in flight: its reply is of no interest any more.
            if this.update(cx, |s, _| s.client.is_none()).unwrap_or(true) {
                return;
            }
            // The columns are set before the first listing line is fetched.
            if result.is_ok()
                && let Ok(set) = this.update(cx, |s, _| {
                    s.call(ida_proto::SetListingOptions { indent, cmt_indent })
                })
                && let Err(e) = set.await
            {
                eprintln!("set_listing_options: {e}");
            }
            this.update(cx, |s, cx| {
                match result {
                    Ok(info) => {
                        s.cursor = info.entry.unwrap_or(info.min_ea);
                        s.history = vec![s.cursor];
                        s.hist_pos = 0;
                        s.info = Some(info);
                        s.phase = Phase::Ready;
                        s.analysis = None;
                        s.dirty = fresh;
                        s.reload_lists(cx);
                        s.reload_marks(cx);
                        s.dbg = DebugModel::new();
                        s.dbg_load_bpts(cx);
                        s.dbg_load_exceptions(cx);
                        cx.emit(SessionEvent::Opened);
                        let ea = s.cursor;
                        cx.emit(SessionEvent::Navigated { ea });
                    }
                    Err(e) => {
                        s.analysis = None;
                        let mut msg = e.to_string();
                        // A host that died while starting (a licence or
                        // install problem) explains itself on stderr.
                        if matches!(e, ida_proto::client::ClientError::HostGone(_)) {
                            let tail: Vec<&str> = s
                                .log
                                .iter()
                                .map(|l| l.trim())
                                .filter(|l| !l.is_empty())
                                .rev()
                                .take(6)
                                .collect();
                            if !tail.is_empty() {
                                msg.push('\n');
                                msg.push_str(
                                    &tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
                                );
                            }
                        }
                        s.phase = Phase::Failed(msg.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Sends a request; resolves with an error if no host is running.
    pub fn call<M: Method>(
        &self,
        m: M,
    ) -> impl std::future::Future<Output = Result<M::Reply, ClientError>> + 'static + use<M> {
        let fut = self.client.as_ref().map(|c| c.call(m));
        async move {
            match fut {
                Some(f) => f.await,
                None => Err(ClientError::HostGone("no database is open".into())),
            }
        }
    }

    /// Runs a mutating request; on success marks the database dirty and
    /// tells every view to refresh.
    pub fn mutate<M: Method>(&mut self, m: M, cx: &mut Context<Self>) {
        let what = M::NAME;
        let fut = self.call(m);
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(_) => s.changed(cx),
                Err(e) => s.error(format!("{what}: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.revision += 1;
        self.reload_lists(cx);
        cx.emit(SessionEvent::Changed);
        cx.notify();
    }

    /// Applies new listing columns to the open database and has the views
    /// refetch. This is not an edit, so the database stays clean.
    pub fn set_listing_options(&mut self, indent: u32, cmt_indent: u32, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        let fut = self.call(ida_proto::SetListingOptions { indent, cmt_indent });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(()) => {
                    s.revision += 1;
                    cx.emit(SessionEvent::Changed);
                    cx.notify();
                }
                Err(e) => s.error(format!("listing options: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// The operand under the caret and how the instruction's operands look.
    pub fn ops(&self) -> crate::operands::Ops {
        crate::operands::Ops {
            rows: self.op_repr.clone(),
            op: self.caret.operand.unwrap_or(0),
        }
    }

    pub fn error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        let msg = msg.into();
        self.log.push(format!("error: {msg}").into());
        self.set_status(msg, true, cx);
    }

    pub fn info_msg(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.set_status(msg.into(), false, cx);
    }

    /// Shows `msg` in the status bar until the TTL passes or a command runs.
    pub fn set_status(&mut self, msg: SharedString, error: bool, cx: &mut Context<Self>) {
        self.status.set(msg, error, Instant::now());
        cx.notify();
        cx.spawn(async move |this, cx| {
            // A little late: a timer that fires early would find the message still live.
            cx.background_executor()
                .timer(STATUS_TTL + Duration::from_millis(200))
                .await;
            this.update(cx, |s, cx| {
                if s.status.expire(Instant::now()) {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The next command: the previous message has been read.
    pub fn clear_status(&mut self, cx: &mut Context<Self>) {
        if self.status.get(Instant::now()).is_some() {
            self.status.clear();
            cx.notify();
        }
    }

    /// A user-initiated jump: recorded in history.
    pub fn navigate(&mut self, ea: Ea, cx: &mut Context<Self>) {
        if self.history.get(self.hist_pos) != Some(&self.cursor) {
            self.history.truncate(self.hist_pos + 1);
            self.history.push(self.cursor);
            self.hist_pos = self.history.len() - 1;
        }
        self.history.truncate(self.hist_pos + 1);
        if self.history.last() != Some(&ea) {
            self.history.push(ea);
        }
        self.hist_pos = self.history.len() - 1;
        self.cursor = ea;
        self.status.clear();
        cx.emit(SessionEvent::Navigated { ea });
        cx.notify();
    }

    pub fn set_cursor(&mut self, ea: Ea, origin: Origin, cx: &mut Context<Self>) {
        if self.cursor != ea {
            self.cursor = ea;
            // Keep the current history slot pointing at where we are.
            if let Some(slot) = self.history.get_mut(self.hist_pos) {
                *slot = ea;
            }
            cx.emit(SessionEvent::CursorMoved { ea, origin });
            cx.notify();
        }
    }

    pub fn can_back(&self) -> bool {
        self.hist_pos > 0
    }

    pub fn can_forward(&self) -> bool {
        self.hist_pos + 1 < self.history.len()
    }

    pub fn set_caret(&mut self, caret: Caret, cx: &mut Context<Self>) {
        if self.caret != caret {
            self.caret = caret;
            cx.emit(SessionEvent::CaretChanged);
            cx.notify();
        }
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if self.hist_pos > 0 {
            self.hist_pos -= 1;
            let ea = self.history[self.hist_pos];
            self.cursor = ea;
            cx.emit(SessionEvent::Navigated { ea });
            cx.notify();
        }
    }

    pub fn forward(&mut self, cx: &mut Context<Self>) {
        if self.hist_pos + 1 < self.history.len() {
            self.hist_pos += 1;
            let ea = self.history[self.hist_pos];
            self.cursor = ea;
            cx.emit(SessionEvent::Navigated { ea });
            cx.notify();
        }
    }

    /// Resolves `expr` (address, name or expression) and jumps there.
    pub fn goto_expr(&mut self, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_owned();
        if expr.is_empty() {
            return;
        }
        let fut = self.call(ida_proto::Resolve {
            expr: expr.clone(),
            from: self.cursor,
        });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(Some(ea)) => s.navigate(ea, cx),
                // IDA cannot resolve demangled names; look them up ourselves.
                Ok(None) => match s.lookup_name(&expr) {
                    Some(ea) => s.navigate(ea, cx),
                    None => s.error(format!("'{expr}' is not an address or a name"), cx),
                },
                Err(e) => s.error(e.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Finds a function or name by its display name: exact match first,
    /// then ignoring case, then ignoring a trailing argument list.
    pub fn lookup_name(&self, name: &str) -> Option<Ea> {
        let all = self
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f.start))
            .chain(self.names.iter().map(|n| (n.name.as_str(), n.ea)));
        let bare = |s: &str| s.split('(').next().unwrap_or(s).trim().to_owned();
        let want = bare(name);
        let mut ci = None;
        let mut bare_hit = None;
        for (n, ea) in all {
            if n == name {
                return Some(ea);
            }
            if ci.is_none() && n.eq_ignore_ascii_case(name) {
                ci = Some(ea);
            }
            if bare_hit.is_none() && bare(n).eq_ignore_ascii_case(&want) {
                bare_hit = Some(ea);
            }
        }
        ci.or(bare_hit)
    }

    pub fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        initial: impl Into<String>,
        on_submit: impl Fn(String, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.emit(SessionEvent::Prompt(PromptRequest {
            title: title.into(),
            placeholder: SharedString::default(),
            initial: initial.into(),
            on_submit: Rc::new(on_submit),
            confirm: "OK".into(),
            name: false,
        }));
    }

    /// A prompt for a name (validated as typed), confirmed with "Rename".
    pub fn prompt_name(
        &mut self,
        title: impl Into<SharedString>,
        initial: impl Into<String>,
        on_submit: impl Fn(String, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.emit(SessionEvent::Prompt(PromptRequest {
            title: title.into(),
            placeholder: SharedString::default(),
            initial: initial.into(),
            on_submit: Rc::new(on_submit),
            confirm: "Rename".into(),
            name: true,
        }));
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let fut = self.call(ida_proto::Save {});
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(()) => {
                    s.dirty = false;
                    s.info_msg("Database saved", cx);
                }
                Err(e) => s.error(format!("save: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn undo(&mut self, redo: bool, cx: &mut Context<Self>) {
        let fut = if redo {
            futures::future::Either::Left(self.call(ida_proto::Redo {}))
        } else {
            futures::future::Either::Right(self.call(ida_proto::Undo {}))
        };
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(true) => s.changed(cx),
                Ok(false) => s.info_msg(
                    if redo {
                        "Nothing to redo"
                    } else {
                        "Nothing to undo"
                    },
                    cx,
                ),
                Err(e) => s.error(e.to_string(), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Ends a host that is stuck inside a call that cannot be interrupted.
    /// The database is gone without a save; the tab shows why.
    pub fn kill_host(&mut self, cx: &mut Context<Self>) {
        let idb = self.info.as_ref().map(|i| i.idb_path.clone());
        if let Some(c) = self.client.take() {
            c.kill();
        }
        // The killed host leaves its unpacked working files behind; IDA
        // would pick them up as if they were a crashed session's. Only when a
        // packed database exists, though: without one the unpacked files may
        // be the user's only copy.
        if let Some(idb) = idb.map(std::path::PathBuf::from)
            && idb.is_file()
        {
            std::thread::spawn(move || remove_unpacked(&idb));
        }
        self.phase = Phase::Failed(
            "The IDA host was killed. Unsaved changes were lost; open the file again to continue."
                .into(),
        );
        self.info = None;
        self.dirty = false;
        self.analysis = None;
        self.python_running = false;
        self.ext_pending = false;
        self.ext_task = None;
        self.dbg = DebugModel::new();
        self.marks.clear();
        cx.emit(SessionEvent::Closed);
        cx.notify();
    }

    /// Closes the database, saving if asked, and resolves once the host has
    /// acknowledged and exited. A host that does not answer within
    /// `CLOSE_TIMEOUT` is killed so it cannot block the caller forever.
    pub fn close(&mut self, save: bool, cx: &mut Context<Self>) -> Task<()> {
        let analyzing = self.analyzing();
        let client = self.client.take();
        // A running analysis is asked to stop between two steps, so the host
        // reaches the close request in moments instead of after the whole run.
        if analyzing && let Some(c) = &client {
            drop(c.call(ida_proto::AnalysisStop {}));
        }
        let fut = client.as_ref().map(|c| c.call(ida_proto::Close { save }));
        self.phase = Phase::Empty;
        self.info = None;
        self.dirty = false;
        self.analysis = None;
        self.python_running = false;
        self.ext_pending = false;
        self.ext_task = None;
        self.dbg = DebugModel::new();
        cx.emit(SessionEvent::Closed);
        cx.notify();
        let executor = cx.background_executor().clone();
        executor.clone().spawn(async move {
            let (Some(client), Some(fut)) = (client, fut) else {
                return;
            };
            let timer = executor.timer(CLOSE_TIMEOUT);
            let acked = matches!(
                futures::future::select(Box::pin(fut), timer).await,
                futures::future::Either::Left((Ok(()), _))
            );
            if !acked || !client.wait_exit(Duration::from_secs(5)) {
                client.kill();
            }
        })
    }

    pub fn file_name(&self) -> Option<String> {
        let info = self.info.as_ref()?;
        let path = std::path::Path::new(&info.input_path);
        Some(path.file_name()?.to_string_lossy().into_owned())
    }
}

/// Deletes `<db>.id0/.id1/.id2/.nam/.til`, retrying while the dying host
/// still holds them.
fn remove_unpacked(idb: &std::path::Path) {
    for _ in 0..25 {
        let mut left = false;
        for ext in ["id0", "id1", "id2", "nam", "til"] {
            let p = idb.with_extension(ext);
            if p.exists() && std::fs::remove_file(&p).is_err() {
                left = true;
            }
        }
        if !left {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(c) = self.client.take() {
            c.kill();
        }
    }
}

/// For a forced processor, makes sure the private-extension copy of the input
/// exists and matches the original. Other opens need no preparation.
fn prepare_input(req: &OpenRequest) -> Result<(), String> {
    let input = req.input_path();
    if input == req.path {
        return Ok(());
    }
    let same = std::fs::metadata(&input)
        .ok()
        .zip(std::fs::metadata(&req.path).ok())
        .is_some_and(|(a, b)| a.len() == b.len());
    if !same {
        std::fs::copy(&req.path, &input).map_err(|e| {
            format!(
                "cannot copy {} to {}: {e}",
                req.path.display(),
                input.display()
            )
        })?;
    }
    Ok(())
}

/// The host for `req`: the Binary loader and a private IDAUSR holding the
/// processor map when needed.
fn host_config(req: &OpenRequest) -> HostConfig {
    let cfg = HostConfig::discover();
    if req.loader != Loader::Binary {
        return cfg;
    }
    let names: Vec<&str> = PROCESSORS
        .iter()
        .map(|(p, _)| *p)
        .filter(|p| *p != "metapc")
        .collect();
    let dir = settings::config_dir().join("idausr");
    cfg.clone().raw_binary(&dir, &names).unwrap_or_else(|e| {
        eprintln!("cannot write the processor map in {}: {e}", dir.display());
        cfg
    })
}
