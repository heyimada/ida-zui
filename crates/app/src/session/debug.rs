// The debugger half of a session: host `debug` events update the model in
// `debugger.rs`, and the debugger commands (start, resume, breakpoints,
// register edits) are requests whose results land there too. Views repaint
// on `SessionEvent::DebugChanged`.
use super::{Session, SessionEvent};
use crate::debugger::{LogKind, StopException, dump_rows, stack_words};
use crate::settings::{self, ExceptionPolicy};
use gpui::Context;
use ida_proto::*;

/// Stack words shown below the stack pointer.
const STACK_WORDS: usize = 48;
/// Bytes the memory panel reads.
pub const MEM_BYTES: u32 = 256;
/// How long a pause may take before the UI offers to terminate.
const PAUSE_TIMEOUT_MS: u32 = 3000;

impl Session {
    /// A `debug` event from the host.
    pub(super) fn on_debug_event(&mut self, ev: DebugEvent, cx: &mut Context<Self>) {
        match ev {
            DebugEvent::State {
                state,
                ip,
                tid,
                reason,
            } => {
                self.dbg.state = state;
                self.dbg.tid = tid;
                self.dbg.reason = reason.clone();
                match state {
                    DbgRun::Suspended => {
                        self.dbg.ip = ip;
                        self.dbg_fetch_stop(ip, cx);
                    }
                    DbgRun::Running => {
                        self.dbg.ip = None;
                        self.dbg.exception = None;
                    }
                    DbgRun::None => {
                        self.dbg.clear_process();
                        self.dbg.log(LogKind::Info, "The process has ended");
                    }
                }
                self.dbg_changed(cx);
            }
            DebugEvent::ProcessStarted { pid, name, .. }
            | DebugEvent::ProcessAttached { pid, name, .. } => {
                self.dbg.pid = pid;
                self.dbg
                    .log(LogKind::Info, format!("Process {pid} started: {name}"));
                self.dbg_changed(cx);
            }
            DebugEvent::ProcessExited { pid, exit_code } => {
                self.dbg.log(
                    LogKind::Info,
                    format!("Process {pid} exited with code {exit_code}"),
                );
                self.dbg_changed(cx);
            }
            DebugEvent::ProcessDetached { pid } => {
                self.dbg
                    .log(LogKind::Info, format!("Detached from process {pid}"));
                self.dbg_changed(cx);
            }
            DebugEvent::ThreadStarted { tid, .. } => {
                self.dbg.log(LogKind::Info, format!("Thread {tid} started"));
            }
            DebugEvent::ThreadExited { tid, exit_code } => {
                self.dbg.log(
                    LogKind::Info,
                    format!("Thread {tid} exited with code {exit_code}"),
                );
            }
            DebugEvent::LibLoaded { name, base, .. } => {
                self.dbg
                    .log(LogKind::Info, format!("Loaded {name} at 0x{base:X}"));
            }
            DebugEvent::LibUnloaded { name } => {
                self.dbg.log(LogKind::Info, format!("Unloaded {name}"));
            }
            DebugEvent::Breakpoint { ea, hits, .. } => {
                self.dbg.log(
                    LogKind::Info,
                    format!("Breakpoint hit at 0x{ea:X} ({hits})"),
                );
            }
            DebugEvent::Step { .. } => {}
            DebugEvent::Exception {
                ea,
                code,
                name,
                info,
                stopped,
                first_chance,
                ..
            } => {
                let chance = if first_chance {
                    "first chance"
                } else {
                    "last chance"
                };
                let label = if name.is_empty() {
                    format!("0x{code:08X}")
                } else {
                    name.clone()
                };
                self.dbg.log(
                    if stopped {
                        LogKind::Warn
                    } else {
                        LogKind::Info
                    },
                    format!(
                        "Exception {label} at 0x{ea:X} ({chance}){}: {info}",
                        if stopped {
                            ""
                        } else {
                            ", passed to the program"
                        }
                    ),
                );
                if stopped {
                    self.dbg.exception = Some(StopException {
                        code,
                        name,
                        first_chance,
                    });
                }
                self.dbg_changed(cx);
            }
            DebugEvent::Output { text, .. } => {
                for line in text.lines() {
                    self.dbg.log(LogKind::Output, line);
                }
                self.dbg_changed(cx);
            }
            DebugEvent::Rebased {
                delta, restored, ..
            } => self.dbg_rebased(delta, restored, cx),
            DebugEvent::PauseTimeout => {
                self.dbg.log(
                    LogKind::Error,
                    "The process did not stop after the pause request",
                );
                self.error(
                    "The process did not stop. Terminate it from the Debugger menu if it hangs",
                    cx,
                );
                self.dbg_changed(cx);
            }
            DebugEvent::Error { text } => {
                self.dbg.log(LogKind::Error, text.clone());
                self.error(text, cx);
                self.dbg_changed(cx);
            }
        }
    }

    pub fn dbg_changed(&mut self, cx: &mut Context<Self>) {
        self.dbg.revision += 1;
        cx.emit(SessionEvent::DebugChanged);
        cx.notify();
    }

    /// The kernel moved the database base: every address the UI holds shifts
    /// by `delta`.
    fn dbg_rebased(&mut self, delta: i64, restored: bool, cx: &mut Context<Self>) {
        use crate::debugger::shift;
        for e in &mut self.history {
            *e = shift(*e, delta);
        }
        self.cursor = shift(self.cursor, delta);
        self.dbg.log(
            LogKind::Info,
            if restored {
                format!("Database rebased back by {delta:+#X}")
            } else {
                format!("Database rebased by {delta:+#X} to match the process")
            },
        );
        let (info, bpts) = (self.call(Info {}), self.call(DbgBpts {}));
        cx.spawn(async move |this, cx| {
            let info = info.await;
            let bpts = bpts.await;
            this.update(cx, |s, cx| {
                if let Ok(info) = info {
                    s.info = Some(info);
                }
                if let Ok(b) = bpts {
                    s.dbg.bpts = b;
                }
                s.revision += 1;
                s.reload_lists(cx);
                cx.emit(SessionEvent::Rebased { delta });
                let ea = s.cursor;
                cx.emit(SessionEvent::Navigated { ea });
                s.dbg_changed(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The process stopped: read the registers, stack and the rest, then move
    /// the views to the instruction pointer.
    fn dbg_fetch_stop(&mut self, ip: Option<Ea>, cx: &mut Context<Self>) {
        let regs = self.call(DbgRegs { all: false });
        let frames = self.call(DbgStack {});
        let threads = self.call(DbgThreads {});
        let modules = self.call(DbgModules {});
        let bpts = self.call(DbgBpts {});
        let word = if self.info.as_ref().is_some_and(|i| i.bitness == 32) {
            4
        } else {
            8
        };
        let mem_expr = self.dbg.mem_expr.clone();
        cx.spawn(async move |this, cx| {
            let regs = regs.await;
            let frames = frames.await;
            let threads = threads.await;
            let modules = modules.await;
            let bpts = bpts.await;
            // The stack words, symbolized.
            let sp = regs
                .as_ref()
                .ok()
                .and_then(|r| r.iter().find(|r| r.sp).and_then(DbgReg::value));
            let mut stack = Vec::new();
            if let Some(sp) = sp
                && let Ok(fut) = this.update(cx, |s, _| {
                    s.call(DbgMemRead {
                        ea: sp,
                        len: (STACK_WORDS * word) as u32,
                    })
                })
                && let Ok(mem) = fut.await
            {
                stack = stack_words(sp, &mem.bytes(), word);
                let eas: Vec<Ea> = stack.iter().filter_map(|w| w.value).collect();
                if let Ok(fut) = this.update(cx, |s, _| s.call(DbgSymbolize { eas: eas.clone() }))
                    && let Ok(names) = fut.await
                {
                    let mut names = names.into_iter();
                    for w in stack.iter_mut().filter(|w| w.value.is_some()) {
                        w.sym = names.next().flatten();
                    }
                }
            }
            let mem = Self::read_mem(&this, &mem_expr, ip, cx).await;
            this.update(cx, |s, cx| {
                if let Ok(r) = regs {
                    s.dbg.set_regs(r);
                }
                if let Ok(f) = frames {
                    s.dbg.frames = f;
                }
                if let Ok(t) = threads {
                    s.dbg.threads = t;
                }
                if let Ok(m) = modules {
                    s.dbg.modules = m;
                }
                if let Ok(b) = bpts {
                    s.dbg.bpts = b;
                }
                s.dbg.stack = stack;
                s.dbg.sp = sp;
                s.dbg.mem_addr = mem.as_ref().map(|m| m.ea);
                s.dbg.mem = mem;
                let what = match s.dbg.exception.as_ref() {
                    Some(e) if s.dbg.reason == "exception" => {
                        let label = if e.name.is_empty() {
                            format!("exception 0x{:08X}", e.code)
                        } else {
                            e.name.clone()
                        };
                        format!(
                            "{label}{}",
                            if e.first_chance { "" } else { " (last chance)" }
                        )
                    }
                    _ => s.dbg.reason.clone(),
                };
                if let Some(ip) = ip {
                    let at = s.symbol_at(ip);
                    s.status = Some((format!("Suspended at {at} ({what})").into(), false));
                    s.cursor = ip;
                    cx.emit(SessionEvent::DebugStopped);
                    cx.emit(SessionEvent::Navigated { ea: ip });
                }
                s.dbg_changed(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reads the memory the panel follows: evaluates `expr` (an IDC
    /// expression, so registers work) and dumps `MEM_BYTES` from there.
    async fn read_mem(
        this: &gpui::WeakEntity<Self>,
        expr: &str,
        ip: Option<Ea>,
        cx: &mut gpui::AsyncApp,
    ) -> Option<DbgMem> {
        let eval = if expr.trim().is_empty() {
            None
        } else {
            let e = expr.to_owned();
            this.update(cx, |s, _| s.call(DbgEval { expr: e })).ok()
        };
        let ea = match eval {
            Some(f) => f.await.ok(),
            None => ip,
        }?;
        this.update(cx, |s, _| s.call(DbgMemRead { ea, len: MEM_BYTES }))
            .ok()?
            .await
            .ok()
    }

    /// `name+0xN` (or the address) for the status line.
    pub fn symbol_at(&self, ea: Ea) -> String {
        match self.function_at(ea) {
            Some(f) if f.start == ea => f.name.clone(),
            Some(f) => format!("{}+0x{:X}", f.name, ea - f.start),
            None => format!("0x{ea:X}"),
        }
    }

    // ---- commands --------------------------------------------------------

    /// F9: starts the process, or continues a suspended one.
    pub fn dbg_start_or_continue(&mut self, cx: &mut Context<Self>) {
        match self.dbg.state {
            DbgRun::None => self.dbg_start(cx),
            DbgRun::Suspended => self.dbg_resume(ResumeHow::Continue, 0, cx),
            DbgRun::Running => self.info_msg("The process is running", cx),
        }
    }

    pub fn dbg_start(&mut self, cx: &mut Context<Self>) {
        if !self.is_ready() || self.dbg.active() {
            return;
        }
        let d = settings::get(cx).debugger.clone();
        let options = DbgOptions {
            stop_at_entry: d.stop_at_entry,
            stop_on_library: d.stop_on_library,
            stop_on_thread: d.stop_on_thread,
            disable_aslr: d.disable_aslr,
        };
        let path = (!d.app_path.trim().is_empty()).then(|| d.app_path.trim().to_owned());
        self.dbg.log(LogKind::Info, "Starting the process");
        self.dbg.exception = None;
        self.status = None;
        let list = (d.exceptions != ExceptionPolicy::Ida).then(|| self.call(DbgExceptions {}));
        cx.spawn(async move |this, cx| {
            // The exception policy goes in before the process runs.
            if let Some(list) = list
                && let Ok(list) = list.await
            {
                for e in list {
                    let (stop, pass) = match d.exceptions {
                        ExceptionPolicy::PassAll => (false, true),
                        _ => (true, e.pass),
                    };
                    if (stop, pass) == (e.stop, e.pass) {
                        continue;
                    }
                    if let Ok(f) = this.update(cx, |s, _| {
                        s.call(DbgSetException {
                            code: e.code,
                            stop,
                            pass,
                        })
                    }) {
                        let _ = f.await;
                    }
                }
            }
            let Ok(fut) = this.update(cx, |s, _| {
                s.call(DbgStart {
                    path,
                    args: d.args.clone(),
                    cwd: d.cwd.clone(),
                    options,
                })
            }) else {
                return;
            };
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(st) => {
                    s.dbg.state = st.state;
                    s.dbg.pid = st.pid;
                    s.dbg_changed(cx);
                }
                Err(e) => {
                    s.dbg.log(LogKind::Error, format!("Cannot start: {e}"));
                    s.error(format!("debugger: {e}"), cx);
                    s.dbg_changed(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn dbg_resume(&mut self, how: ResumeHow, ea: Ea, cx: &mut Context<Self>) {
        if !self.dbg.suspended() {
            return;
        }
        let fut = self.call(DbgResume { how, ea });
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |s, cx| s.error(format!("debugger: {e}"), cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn dbg_pause(&mut self, cx: &mut Context<Self>) {
        if self.dbg.state != DbgRun::Running {
            return;
        }
        let fut = self.call(DbgPause {
            timeout_ms: PAUSE_TIMEOUT_MS,
        });
        self.info_msg("Pausing the process…", cx);
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |s, cx| s.error(format!("debugger: {e}"), cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn dbg_terminate(&mut self, cx: &mut Context<Self>) {
        if !self.dbg.active() {
            return;
        }
        let fut = self.call(DbgTerminate {});
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |s, cx| s.error(format!("debugger: {e}"), cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn dbg_detach(&mut self, cx: &mut Context<Self>) {
        if !self.dbg.active() {
            return;
        }
        let fut = self.call(DbgDetach {});
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |s, cx| s.error(format!("debugger: {e}"), cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn dbg_attach(&mut self, pid: u32, cx: &mut Context<Self>) {
        let d = settings::get(cx).debugger.clone();
        let options = DbgOptions {
            stop_at_entry: false,
            stop_on_library: d.stop_on_library,
            stop_on_thread: d.stop_on_thread,
            disable_aslr: false,
        };
        let fut = self.call(DbgAttach { pid, options });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(st) => {
                    s.dbg.state = st.state;
                    s.dbg.pid = st.pid;
                    s.dbg_changed(cx);
                }
                Err(e) => s.error(format!("attach: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Reads the breakpoints stored in the database.
    pub fn dbg_load_bpts(&mut self, cx: &mut Context<Self>) {
        let fut = self.call(DbgBpts {});
        cx.spawn(async move |this, cx| {
            if let Ok(list) = fut.await {
                this.update(cx, |s, cx| {
                    s.dbg.bpts = list;
                    s.dbg_changed(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn dbg_bpts_result(
        &mut self,
        fut: impl std::future::Future<Output = Result<Vec<DbgBpt>, ClientError>> + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(list) => {
                    s.dbg.bpts = list;
                    s.dbg_changed(cx);
                }
                Err(e) => s.error(format!("breakpoint: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// F2: sets or removes the breakpoint at `ea`.
    pub fn dbg_toggle_bpt(&mut self, ea: Ea, cx: &mut Context<Self>) {
        if !self.is_ready() {
            return;
        }
        if self.dbg.bpt_at(ea).is_some() {
            let fut = self.call(DbgBptDel { ea });
            self.dbg_bpts_result(fut, cx);
        } else {
            let fut = self.call(DbgBptAdd {
                ea,
                kind: "soft".into(),
                size: 0,
                cond: String::new(),
                lang: String::new(),
            });
            self.dbg_bpts_result(fut, cx);
        }
    }

    pub fn dbg_remove_bpt(&mut self, ea: Ea, cx: &mut Context<Self>) {
        let fut = self.call(DbgBptDel { ea });
        self.dbg_bpts_result(fut, cx);
    }

    pub fn dbg_bpt_enable(&mut self, ea: Ea, enabled: bool, cx: &mut Context<Self>) {
        let fut = self.call(DbgBptEnable { ea, enabled });
        self.dbg_bpts_result(fut, cx);
    }

    pub fn dbg_bpt_cond(&mut self, ea: Ea, cond: String, lang: String, cx: &mut Context<Self>) {
        let fut = self.call(DbgBptCond { ea, cond, lang });
        self.dbg_bpts_result(fut, cx);
    }

    /// Evaluates `expr` and writes it to register `name`, then rereads the
    /// registers so the change shows highlighted.
    pub fn dbg_set_reg(&mut self, name: String, expr: String, cx: &mut Context<Self>) {
        let eval = self.call(DbgEval { expr });
        cx.spawn(async move |this, cx| {
            let result = match eval.await {
                Ok(value) => match this.update(cx, |s, _| s.call(DbgSetReg { name, value })) {
                    Ok(f) => f.await.map_err(|e| e.to_string()),
                    Err(_) => return,
                },
                Err(e) => Err(e.to_string()),
            };
            this.update(cx, |s, cx| match result {
                Ok(()) => {
                    let ip = s.dbg.ip;
                    s.dbg_fetch_stop_keep_view(ip, cx);
                }
                Err(e) => s.error(format!("register: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// Rereads the stop state without moving the views (after an edit).
    pub fn dbg_fetch_stop_keep_view(&mut self, ip: Option<Ea>, cx: &mut Context<Self>) {
        let regs = self.call(DbgRegs { all: false });
        let frames = self.call(DbgStack {});
        let mem_expr = self.dbg.mem_expr.clone();
        cx.spawn(async move |this, cx| {
            let regs = regs.await;
            let frames = frames.await;
            let mem = Self::read_mem(&this, &mem_expr, ip, cx).await;
            this.update(cx, |s, cx| {
                if let Ok(r) = regs {
                    // The stop's own snapshot stays the baseline for highlighting.
                    let prev = std::mem::take(&mut s.dbg.prev_regs);
                    s.dbg.set_regs(r);
                    s.dbg.changed = crate::debugger::changed_regs(&prev, &s.dbg.regs);
                    s.dbg.prev_regs = prev;
                    s.dbg.ip = s.dbg.reg_ip().or(s.dbg.ip);
                }
                if let Ok(f) = frames {
                    s.dbg.frames = f;
                }
                if mem.is_some() {
                    s.dbg.mem_addr = mem.as_ref().map(|m| m.ea);
                    s.dbg.mem = mem;
                }
                s.dbg_changed(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn dbg_select_thread(&mut self, tid: u32, cx: &mut Context<Self>) {
        let fut = self.call(DbgSelectThread { tid });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(st) => {
                    s.dbg.tid = st.tid;
                    s.dbg.ip = st.ip;
                    s.dbg_fetch_stop(st.ip, cx);
                }
                Err(e) => s.error(format!("thread: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// The memory panel follows `expr` from now on.
    pub fn dbg_set_mem_expr(&mut self, expr: String, cx: &mut Context<Self>) {
        self.dbg.mem_expr = expr.clone();
        if !self.dbg.suspended() {
            self.dbg_changed(cx);
            return;
        }
        let ip = self.dbg.ip;
        cx.spawn(async move |this, cx| {
            let mem = Self::read_mem(&this, &expr, ip, cx).await;
            this.update(cx, |s, cx| {
                match mem {
                    Some(m) => {
                        s.dbg.mem_addr = Some(m.ea);
                        s.dbg.mem = Some(m);
                    }
                    None => {
                        s.dbg.mem = None;
                        s.error(format!("cannot evaluate '{}'", s.dbg.mem_expr), cx);
                    }
                }
                s.dbg_changed(cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn dbg_load_exceptions(&mut self, cx: &mut Context<Self>) {
        let fut = self.call(DbgExceptions {});
        cx.spawn(async move |this, cx| {
            if let Ok(list) = fut.await {
                this.update(cx, |s, cx| {
                    s.dbg.exceptions = list;
                    s.dbg_changed(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn dbg_set_exception(&mut self, code: u32, stop: bool, pass: bool, cx: &mut Context<Self>) {
        let fut = self.call(DbgSetException { code, stop, pass });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |s, cx| match result {
                Ok(list) => {
                    s.dbg.exceptions = list;
                    s.dbg_changed(cx);
                }
                Err(e) => s.error(format!("exception: {e}"), cx),
            })
            .ok();
        })
        .detach();
    }
}

/// Rows of the memory dump, 16 bytes each.
pub fn mem_rows(mem: &DbgMem) -> Vec<crate::debugger::DumpRow> {
    dump_rows(mem.ea, &mem.bytes(), 16)
}
