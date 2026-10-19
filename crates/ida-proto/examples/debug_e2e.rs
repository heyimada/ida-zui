//! End-to-end check of the debugger against a program we compile ourselves.
//!
//! Usage: cargo run -p ida-proto --example debug_e2e -- <izs_debuggee.exe> [artifact-dir]
//! (`scripts/e2e.ps1` builds the target with `scripts/build-debuggee.ps1`).
//!
//! The target (`scripts/debuggee/debuggee.c`) loops over two small functions,
//! prints, raises one caught exception and exits with code 7. The sessions
//! below start it through `ida-host.exe`, stop at breakpoints, read and write
//! registers and memory, step, run to an address, check conditional
//! breakpoints, pause a spinning process, exception handling and rebase
//! handling (the database must come back to its original base and a saved
//! database must reopen at it). Output: `<artifact-dir>/debug.txt`, which holds
//! no addresses so it can be diffed across runs.

use ida_proto::client::HostConfig;
use ida_proto::*;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Kills a process we started when the test ends or panics.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

struct Sess {
    client: Client,
    events: futures::channel::mpsc::UnboundedReceiver<HostEvent>,
    /// Debug events since the session was created, in order.
    log: Vec<DebugEvent>,
    t0: Instant,
}

impl Sess {
    fn spawn(t0: Instant) -> Sess {
        let (client, logs, events) =
            Client::spawn_with_events(&HostConfig::discover()).expect("spawn host");
        // Drain the stderr log so the host never blocks on it.
        std::thread::spawn(move || futures::executor::block_on_stream(logs).for_each(drop));
        Sess {
            client,
            events,
            log: Vec::new(),
            t0,
        }
    }

    fn call<M: Method>(&self, m: M) -> M::Reply {
        match self.client.call_blocking(m) {
            Ok(r) => r,
            Err(e) => panic!("{} failed: {e}", M::NAME),
        }
    }

    fn try_call<M: Method>(&self, m: M) -> Result<M::Reply, ClientError> {
        self.client.call_blocking(m)
    }

    /// Moves queued host events into `log`.
    fn pump(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            if let HostEvent::Debug(d) = ev {
                self.log.push(d);
            }
        }
    }

    /// Waits until `pred` accepts a debug event that arrived after `from`
    /// (an index into `log`); returns the index of that event.
    fn wait(
        &mut self,
        from: usize,
        what: &str,
        secs: u64,
        pred: impl Fn(&DebugEvent) -> bool,
    ) -> usize {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            self.pump();
            if let Some(i) = self.log[from..].iter().position(&pred) {
                return from + i;
            }
            if Instant::now() > deadline {
                panic!(
                    "timed out waiting for {what}; events: {:?}",
                    &self.log[from..]
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits for the next stop (state suspended) after `from`; returns its ip.
    fn wait_stop(&mut self, from: usize, what: &str) -> (usize, Ea, DebugEvent) {
        let i = self.wait(from, what, 30, |e| {
            matches!(
                e,
                DebugEvent::State {
                    state: DbgRun::Suspended,
                    ..
                }
            )
        });
        let ev = self.log[i].clone();
        let DebugEvent::State { ip, .. } = &ev else {
            unreachable!()
        };
        (i, ip.expect("a suspended state has an ip"), ev)
    }

    fn reg(&self, name: &str) -> u64 {
        let regs = self.call(DbgRegs { all: false });
        regs.iter()
            .find(|r| r.name == name)
            .and_then(|r| r.value())
            .unwrap_or_else(|| panic!("register {name} missing"))
    }

    fn func(&self, name: &str) -> Ea {
        let funcs = self.call(Functions {});
        funcs
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("function {name} not found"))
            .start
    }

    fn say(&self, text: &str) {
        eprintln!("[{:6.1}s] {text}", self.t0.elapsed().as_secs_f32());
    }
}

fn pid_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

fn kind(e: &DebugEvent) -> &'static str {
    match e {
        DebugEvent::State { .. } => "state",
        DebugEvent::ProcessStarted { .. } => "process_started",
        DebugEvent::ProcessAttached { .. } => "process_attached",
        DebugEvent::ProcessExited { .. } => "process_exited",
        DebugEvent::ProcessDetached { .. } => "process_detached",
        DebugEvent::ThreadStarted { .. } => "thread_started",
        DebugEvent::ThreadExited { .. } => "thread_exited",
        DebugEvent::LibLoaded { .. } => "lib_loaded",
        DebugEvent::LibUnloaded { .. } => "lib_unloaded",
        DebugEvent::Breakpoint { .. } => "breakpoint",
        DebugEvent::Step { .. } => "step",
        DebugEvent::Exception { .. } => "exception",
        DebugEvent::Output { .. } => "output",
        DebugEvent::Rebased { .. } => "rebased",
        DebugEvent::PauseTimeout => "pause_timeout",
        DebugEvent::Error { .. } => "error",
    }
}

fn opts() -> DbgOptions {
    DbgOptions::default()
}

fn start(s: &Sess, args: &str, options: DbgOptions) -> DbgStateInfo {
    s.call(DbgStart {
        path: None,
        args: args.into(),
        cwd: String::new(),
        options,
    })
}

fn open(s: &Sess, exe: &Path) -> DbInfo {
    s.call(Open {
        path: exe.display().to_string(),
        auto_analyze: true,
        base: None,
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let exe_src = PathBuf::from(
        args.next()
            .expect("usage: debug_e2e <izs_debuggee.exe> [artifact-dir]"),
    );
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| "artifacts/e2e".into()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let work = exe_src.parent().unwrap().join("session");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let exe = work.join("izs_debuggee.exe");
    std::fs::copy(&exe_src, &exe).unwrap();

    let t0 = Instant::now();
    let mut out = String::new();
    let mut failures: Vec<String> = Vec::new();
    let mut check = |cond: bool, what: &str| {
        if !cond {
            failures.push(what.to_owned());
        }
        eprintln!("  {} {what}", if cond { "ok  " } else { "FAIL" });
    };

    let mut s = Sess::spawn(t0);
    let info = open(&s, &exe);
    let orig_base = info.image_base;
    writeln!(
        out,
        "== database\nbitness={} processor={}",
        info.bitness, info.processor
    )
    .unwrap();
    s.say("opened");

    // ---- session A: breakpoint, registers, memory, steps, run to ----------
    let leaf = s.func("izs_leaf");
    let outer = s.func("izs_outer");
    check(
        leaf != outer && leaf != 0,
        "the target's functions are in the database",
    );
    // Pseudocode lines carry the addresses of their items (for the markers).
    let pc = s.call(Decompile { ea: outer });
    check(
        pc.line_eas.len() == pc.lines.len()
            && pc
                .line_eas
                .iter()
                .flatten()
                .any(|e| *e >= outer && *e < outer + 0x20),
        "pseudocode lines report the addresses of their ctree items",
    );

    let bpts = s.call(DbgBptAdd {
        ea: leaf,
        kind: "soft".into(),
        size: 0,
        cond: String::new(),
        lang: String::new(),
    });
    check(
        bpts.len() == 1 && bpts[0].ea == leaf && bpts[0].enabled,
        "a breakpoint can be set before the process starts",
    );
    check(
        s.try_call(DbgBptAdd {
            ea: leaf,
            kind: "soft".into(),
            size: 0,
            cond: String::new(),
            lang: String::new(),
        })
        .is_err(),
        "a second breakpoint at the same address is refused",
    );

    let mut mark = s.log.len();
    let st = start(&s, "5", opts());
    s.say("started");
    check(st.state != DbgRun::None, "start returns a live state");
    check(
        st.orig_base == Some(orig_base),
        "the state records the original base",
    );
    let (stop_i, ip, ev) = s.wait_stop(mark, "the first breakpoint");
    let DebugEvent::State { reason, .. } = &ev else {
        unreachable!()
    };
    check(reason == "breakpoint", "the stop reason is a breakpoint");
    writeln!(out, "\n== session A: events to the first stop").unwrap();
    let mut last = "";
    for e in &s.log[mark..=stop_i] {
        let k = kind(e);
        if k != last {
            writeln!(out, "{k}").unwrap();
        }
        last = k;
    }
    let started = s.log[mark..=stop_i].iter().find_map(|e| match e {
        DebugEvent::ProcessStarted { name, base, .. } => Some((name.clone(), *base)),
        _ => None,
    });
    check(started.is_some(), "process_started is reported");
    let rebase = s.log[mark..=stop_i].iter().find_map(|e| match e {
        DebugEvent::Rebased {
            delta, restored, ..
        } => Some((*delta, *restored)),
        _ => None,
    });
    check(
        matches!(rebase, Some((d, false)) if d != 0),
        "ASLR moved the image and the host reported a rebase",
    );
    let state = s.call(DbgState {});
    check(
        state.state == DbgRun::Suspended && state.ip == Some(ip),
        "dbg_state agrees with the stop",
    );
    check(
        state.base != orig_base,
        "the database is rebased while debugging",
    );
    let delta = state.base as i64 - orig_base as i64;
    let leaf_now = s.func("izs_leaf");
    let outer_now = s.func("izs_outer");
    check(
        leaf_now as i64 - leaf as i64 == delta,
        "functions moved with the rebase",
    );
    check(
        ip == leaf_now,
        "RIP equals the breakpoint address (rebased correctly)",
    );
    let bpts = s.call(DbgBpts {});
    check(
        bpts.len() == 1 && bpts[0].ea == leaf_now && bpts[0].hits == 1,
        "the breakpoint moved with the rebase and counted one hit",
    );
    check(
        s.reg("RIP") == leaf_now,
        "the RIP register reads the breakpoint address",
    );
    check(
        s.reg("RCX") == 0,
        "RCX holds the first argument (iteration 0)",
    );
    let regs = s.call(DbgRegs { all: false });
    let efl = regs.iter().find(|r| r.name == "EFL");
    check(
        efl.is_some_and(|r| r.bit_names.iter().any(|(b, n)| *b == 6 && n == "ZF")),
        "the flags register carries IDA's bit names",
    );
    check(
        regs.iter().any(|r| r.sp) && regs.iter().any(|r| r.ip),
        "ip and sp registers are marked",
    );
    let all = s.call(DbgRegs { all: true });
    check(
        all.len() > regs.len(),
        "all=true returns more register classes",
    );

    // Memory: the process memory shows the breakpoint instruction hidden.
    let mem = s.call(DbgMemRead { ea: ip, len: 8 });
    let idb = s.call(Bytes { ea: ip, len: 8 });
    check(
        mem.hex.len() == 16 && mem.hex == idb.hex,
        "memory under our breakpoint reads as the original bytes",
    );
    let hole = s.call(DbgMemRead { ea: 0x10, len: 4 });
    check(hole.hex == "????????", "unreadable memory reads as ??");
    let sp = s.reg("RSP");
    let stack = s.call(DbgMemRead { ea: sp, len: 8 });
    let ret = u64::from_le_bytes(
        stack
            .bytes()
            .into_iter()
            .map(|b| b.unwrap_or(0))
            .collect::<Vec<_>>()
            .try_into()
            .unwrap(),
    );
    check(
        ret > outer_now && ret < outer_now + 0x40,
        "the word at RSP is the return address into izs_outer",
    );
    let sym = s.call(DbgSymbolize {
        eas: vec![leaf_now, ret, 0x10],
    });
    check(
        sym[0].as_deref() == Some("izs_leaf")
            && sym[1]
                .as_deref()
                .is_some_and(|n| n.starts_with("izs_outer+0x"))
            && sym[2].is_none(),
        "addresses are symbolized",
    );
    // Memory write round trip in the red zone below RSP (scratch space).
    let scratch = sp - 0x40;
    s.call(DbgMemWrite {
        ea: scratch,
        hex: "DEADBEEF".into(),
    });
    check(
        s.call(DbgMemRead {
            ea: scratch,
            len: 4,
        })
        .hex == "DEADBEEF",
        "written memory reads back",
    );
    check(
        s.call(DbgEval {
            expr: "RSP + 8".into(),
        }) == sp + 8,
        "IDC expressions see the registers",
    );

    // Threads, modules, call stack.
    let threads = s.call(DbgThreads {});
    check(
        !threads.is_empty() && threads.iter().any(|t| t.current),
        "threads are listed with a current one",
    );
    let mods = s.call(DbgModules {});
    check(
        mods.iter()
            .any(|m| m.name.to_lowercase().ends_with("izs_debuggee.exe"))
            && mods
                .iter()
                .any(|m| m.name.to_lowercase().ends_with("ntdll.dll")),
        "modules include the program and ntdll",
    );
    let frames = s.call(DbgStack {});
    check(
        frames.len() >= 2 && frames[0].ea == leaf_now,
        "the call stack starts at the current instruction",
    );
    check(
        frames.iter().any(|f| f.symbol.starts_with("izs_outer")),
        "the call stack contains izs_outer",
    );
    writeln!(out, "\n== session A: stack").unwrap();
    for f in frames.iter().take(3) {
        writeln!(out, "{}", f.symbol.split('+').next().unwrap_or("")).unwrap();
    }

    // Write a register, step out and read the result.
    s.call(DbgSetReg {
        name: "RCX".into(),
        value: 10,
    });
    check(s.reg("RCX") == 10, "a register write reads back");
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::StepOut,
        ea: 0,
    });
    let (_, ip2, _) = s.wait_stop(mark, "the end of step out");
    check(
        ip2 > outer_now && ip2 < outer_now + 0x40,
        "step out returns into izs_outer",
    );
    check(
        s.reg("RAX") == 21,
        "the changed argument reached the program: leaf(10) returned 21",
    );

    // Step over / into. At the call-return site in izs_outer, step into the
    // next instructions until we leave it, then check step over from there.
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::StepInto,
        ea: 0,
    });
    let (_, ip3, ev) = s.wait_stop(mark, "step into");
    check(ip3 != ip2, "step into moves the instruction pointer");
    check(
        matches!(&ev, DebugEvent::State { reason, .. } if reason == "step"),
        "a step reports reason step",
    );
    let prev = s.reg("RAX");
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::StepOver,
        ea: 0,
    });
    let (_, ip4, _) = s.wait_stop(mark, "step over");
    check(ip4 != ip3, "step over moves the instruction pointer");
    let _ = prev;

    // Run to an address: delete the breakpoint, run to izs_outer's next call.
    s.call(DbgBptDel { ea: leaf_now });
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::RunTo,
        ea: outer_now,
    });
    let (_, ip5, _) = s.wait_stop(mark, "run to");
    check(ip5 == outer_now, "run to stops at the requested address");
    check(
        s.reg("RCX") == 1,
        "run to reached the next loop iteration (RCX == 1)",
    );

    // Continue to the end of the program.
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::Continue,
        ea: 0,
    });
    s.wait(mark, "process exit", 30, |e| {
        matches!(e, DebugEvent::ProcessExited { .. })
    });
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });
    let exit_code = s.log[mark..].iter().find_map(|e| match e {
        DebugEvent::ProcessExited { exit_code, .. } => Some(*exit_code),
        _ => None,
    });
    check(exit_code == Some(7), "the program exited with code 7");
    writeln!(out, "\n== session A: events after continue").unwrap();
    let mut last = "";
    for e in &s.log[mark..] {
        let k = kind(e);
        if k != last {
            writeln!(out, "{k}").unwrap();
        }
        last = k;
    }
    let outputs: Vec<&str> = s.log[..]
        .iter()
        .filter_map(|e| match e {
            DebugEvent::Output { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    check(
        outputs.iter().any(|t| t.contains("izs-debuggee: started")),
        "OutputDebugString text arrives as an output event",
    );
    check(
        s.log.iter().any(|e| matches!(e, DebugEvent::LibLoaded { name, .. } if name.to_lowercase().contains("kernel32"))),
        "library loads are reported",
    );
    check(
        s.log
            .iter()
            .any(|e| matches!(e, DebugEvent::ThreadStarted { .. })),
        "thread starts are reported",
    );
    let after = s.call(DbgState {});
    check(
        after.state == DbgRun::None && after.pid == 0,
        "no process is left",
    );
    check(
        after.base == orig_base,
        "the database base is restored after the exit",
    );
    check(
        s.log
            .iter()
            .any(|e| matches!(e, DebugEvent::Rebased { restored: true, .. })),
        "the restore is reported as a rebase event",
    );
    check(
        s.func("izs_leaf") == leaf,
        "functions are back at their original addresses",
    );

    // ---- session B: conditional breakpoint ---------------------------------
    s.call(DbgBptAdd {
        ea: leaf,
        kind: "soft".into(),
        size: 0,
        cond: "RCX == 3".into(),
        lang: String::new(),
    });
    mark = s.log.len();
    start(&s, "6", opts());
    let (_, ipb, _) = s.wait_stop(mark, "the conditional breakpoint");
    check(
        ipb == s.func("izs_leaf"),
        "the conditional breakpoint stops in izs_leaf",
    );
    check(s.reg("RCX") == 3, "the condition RCX == 3 held at the stop");
    let b = s.call(DbgBpts {});
    check(
        b.len() == 1 && b[0].cond == "RCX == 3" && b[0].hits == 1,
        "the condition is kept and only the matching pass counted",
    );
    // Python condition on the same breakpoint, applied while stopped.
    let cur = b[0].ea;
    s.call(DbgBptCond {
        ea: cur,
        cond: "ida_dbg.get_reg_val('RCX') == 5".into(),
        lang: "Python".into(),
    });
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::Continue,
        ea: 0,
    });
    let (_, _, _) = s.wait_stop(mark, "the Python-conditioned breakpoint");
    check(
        s.reg("RCX") == 5,
        "a Python condition (RCX == 5) stops at iteration 5",
    );
    // Disable: the process runs to the end.
    let b = s.call(DbgBptEnable {
        ea: cur,
        enabled: false,
    });
    check(!b[0].enabled, "a breakpoint can be disabled");
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::Continue,
        ea: 0,
    });
    s.wait(mark, "process exit", 30, |e| {
        matches!(e, DebugEvent::ProcessExited { .. })
    });
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });
    check(
        s.call(DbgState {}).base == orig_base,
        "the base is restored after session B",
    );
    s.call(DbgBptDel {
        ea: s.func("izs_leaf"),
    });

    // ---- session C: exceptions ---------------------------------------------
    let excs = s.call(DbgExceptions {});
    check(!excs.is_empty(), "the exception table is listed");
    mark = s.log.len();
    start(&s, "1 x", opts());
    let (_, ipc, evc) = s.wait_stop(mark, "the exception stop");
    let exc = s.log[mark..].iter().find_map(|e| match e {
        DebugEvent::Exception {
            code, stopped, ea, ..
        } => Some((*code, *stopped, *ea)),
        _ => None,
    });
    writeln!(out, "\n== session C: exception stop").unwrap();
    let _ = (ipc, evc);
    check(
        matches!(exc, Some((0xE000_0001, true, _))),
        "a raised exception stops the debugger with its code",
    );
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::Continue,
        ea: 0,
    });
    s.wait(mark, "process exit", 30, |e| {
        matches!(e, DebugEvent::ProcessExited { .. })
    });
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });
    let excs = s.call(DbgSetException {
        code: 0xE000_0001,
        stop: false,
        pass: true,
    });
    check(
        excs.iter()
            .any(|e| e.code == 0xE000_0001 && !e.stop && e.pass),
        "an exception can be set to pass without stopping",
    );
    mark = s.log.len();
    start(&s, "1 x", opts());
    s.wait(mark, "process exit", 30, |e| {
        matches!(e, DebugEvent::ProcessExited { exit_code: 7, .. })
    });
    check(
        !s.log[mark..].iter().any(|e| {
            matches!(
                e,
                DebugEvent::State {
                    state: DbgRun::Suspended,
                    ..
                }
            )
        }),
        "a passed exception does not stop",
    );
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });

    // An unhandled access violation: a first stop, then the last chance.
    s.call(DbgSetException {
        code: 0xE000_0001,
        stop: true,
        pass: true,
    });
    mark = s.log.len();
    start(&s, "1 crash", opts());
    let _ = s.wait_stop(mark, "the access violation");
    mark = s.log.len();
    s.call(DbgResume {
        how: ResumeHow::Continue,
        ea: 0,
    });
    let _ = s.wait_stop(mark, "the last-chance access violation");
    let avs: Vec<(bool, String, bool)> = s
        .log
        .iter()
        .filter_map(|e| match e {
            DebugEvent::Exception {
                code: 0xC000_0005,
                first_chance,
                desc,
                stopped,
                ..
            } => Some((*first_chance, desc.clone(), *stopped)),
            _ => None,
        })
        .collect();
    check(
        avs.len() == 2 && avs[0].0 && !avs[1].0 && avs.iter().all(|a| a.2),
        "an unhandled access violation stops twice: first chance, then last chance",
    );
    check(
        avs.first()
            .is_some_and(|a| a.1.contains("referenced memory")),
        "an exception carries IDA's description",
    );
    s.call(DbgTerminate {});
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });

    // ---- session D: pause and terminate a spinning process ------------------
    mark = s.log.len();
    start(&s, "1 spin", opts());
    s.wait(mark, "running", 10, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::Running,
                ..
            }
        )
    });
    std::thread::sleep(Duration::from_millis(500));
    mark = s.log.len();
    s.call(DbgPause { timeout_ms: 5000 });
    let paused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        s.wait(mark, "pause", 8, |e| {
            matches!(
                e,
                DebugEvent::State {
                    state: DbgRun::Suspended,
                    ..
                } | DebugEvent::PauseTimeout
            )
        })
    }));
    let pause_ok = match paused {
        Ok(i) => matches!(s.log[i], DebugEvent::State { .. }),
        Err(_) => false,
    };
    writeln!(out, "\n== session D\npause completed: {pause_ok}").unwrap();
    check(pause_ok, "a running process can be paused");
    if pause_ok {
        check(s.reg("RIP") != 0, "registers are readable after a pause");
    }
    s.call(DbgTerminate {});
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });
    check(
        s.call(DbgState {}).base == orig_base,
        "the base is restored after terminate",
    );

    // ---- session E: hardware breakpoint, thread selection ---------------------
    let leaf_now = s.func("izs_leaf");
    s.call(DbgBptAdd {
        ea: leaf_now,
        kind: "exec".into(),
        size: 1,
        cond: String::new(),
        lang: String::new(),
    });
    mark = s.log.len();
    start(&s, "2", opts());
    let (_, ipe, _) = s.wait_stop(mark, "the hardware breakpoint");
    check(
        ipe == s.func("izs_leaf"),
        "a hardware execution breakpoint stops at its address",
    );
    let cur = s
        .call(DbgThreads {})
        .into_iter()
        .find(|t| t.current)
        .unwrap();
    check(
        s.call(DbgSelectThread { tid: cur.tid }).tid == Some(cur.tid),
        "the current thread can be selected",
    );
    s.call(DbgBptDel {
        ea: s.func("izs_leaf"),
    });
    s.call(DbgTerminate {});
    s.wait(mark, "the state none", 30, |e| {
        matches!(
            e,
            DebugEvent::State {
                state: DbgRun::None,
                ..
            }
        )
    });

    // ---- session F: attach to a running process, then detach ------------------
    let child = std::process::Command::new(&exe)
        .args(["1", "spin"])
        .spawn()
        .expect("start the target for the attach test");
    let mut child = KillOnDrop(child);
    std::thread::sleep(Duration::from_millis(500));
    let procs = s.call(DbgProcesses {});
    check(
        procs.iter().any(|p| p.pid == child.0.id()),
        "the process list contains the running target",
    );
    mark = s.log.len();
    let attached = s.try_call(DbgAttach {
        pid: child.0.id(),
        options: opts(),
    });
    check(attached.is_ok(), "attaching to a running process works");
    if attached.is_ok() {
        let (_, ipf, _) = s.wait_stop(mark, "the attach stop");
        check(
            ipf != 0 && s.reg("RIP") == ipf,
            "the attached process is suspended with registers",
        );
        check(
            s.call(DbgModules {})
                .iter()
                .any(|m| m.name.to_lowercase().ends_with("izs_debuggee.exe")),
            "an attached process lists its modules",
        );
        s.call(DbgDetach {});
        s.wait(mark, "the state none", 30, |e| {
            matches!(
                e,
                DebugEvent::State {
                    state: DbgRun::None,
                    ..
                }
            )
        });
        check(
            child.0.try_wait().ok().flatten().is_none(),
            "the detached process is still running",
        );
        check(
            s.call(DbgState {}).base == orig_base,
            "the base is restored after detach",
        );
    }
    let _ = child.0.kill();
    let _ = child.0.wait();

    // ---- save guard, then close/reopen at the original base ------------------
    s.call(DbgBptAdd {
        ea: s.func("izs_leaf"),
        kind: "soft".into(),
        size: 0,
        cond: String::new(),
        lang: String::new(),
    });
    mark = s.log.len();
    start(&s, "1 spin", opts());
    let _ = s.wait_stop(mark, "the breakpoint before the save test");
    check(
        s.try_call(Save {}).is_err(),
        "saving is refused while a process is being debugged",
    );
    check(
        s.call(DbgState {}).base != orig_base,
        "(precondition) the database is rebased during the save test",
    );
    // Close with save while the session is live: the host ends the session,
    // restores the base and only then saves.
    let _ = s.try_call(Close { save: true });
    s.client.wait_exit(Duration::from_secs(60));
    let mut s2 = Sess::spawn(t0);
    let info2 = open(&s2, &exe);
    check(
        info2.image_base == orig_base,
        "the database saved during a live session reopens at its original base",
    );
    let bp2 = s2.call(DbgBpts {});
    check(
        bp2.iter().any(|b| b.ea == s2.func("izs_leaf")),
        "the breakpoint survived the save at its original address",
    );
    s2.pump();
    let _ = s2.try_call(Close { save: false });
    s2.client.wait_exit(Duration::from_secs(30));

    // ---- session G: the debuggee dies with its host ----------------------------
    let s3 = Sess::spawn(t0);
    open(&s3, &exe);
    let st = s3.call(DbgStart {
        path: None,
        args: "1 spin".into(),
        cwd: String::new(),
        options: opts(),
    });
    std::thread::sleep(Duration::from_millis(500));
    let pid = s3.call(DbgState {}).pid.max(st.pid);
    check(
        pid != 0 && pid_alive(pid),
        "(precondition) the debuggee of the last host runs",
    );
    s3.client.kill();
    s3.client.wait_exit(Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(1000));
    check(!pid_alive(pid), "killing the host also ends its debuggee");

    writeln!(out, "\n== checks").unwrap();
    if failures.is_empty() {
        writeln!(out, "all passed").unwrap();
    }
    for f in &failures {
        writeln!(out, "FAILED: {f}").unwrap();
    }
    let path = out_dir.join("debug.txt");
    std::fs::write(&path, &out).unwrap();
    println!("wrote {}", path.display());
    if !failures.is_empty() {
        eprintln!(
            "{} check(s) failed:\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
        std::process::exit(1);
    }
}
