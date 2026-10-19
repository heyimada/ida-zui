//! Debugger types: replies of the `dbg_*` requests and the `debug` events.

use crate::Ea;
use serde::{Deserialize, Serialize};

/// What happens when a suspended process resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeHow {
    Continue,
    StepInto,
    StepOver,
    /// Run until the current function returns.
    StepOut,
    /// Run to the address in `ea`.
    RunTo,
}

impl ResumeHow {
    pub fn code(self) -> i32 {
        match self {
            ResumeHow::Continue => 0,
            ResumeHow::StepInto => 1,
            ResumeHow::StepOver => 2,
            ResumeHow::StepOut => 3,
            ResumeHow::RunTo => 4,
        }
    }
}

/// Session options applied when a process starts or is attached.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DbgOptions {
    pub stop_at_entry: bool,
    pub stop_on_library: bool,
    pub stop_on_thread: bool,
    pub disable_aslr: bool,
}

impl DbgOptions {
    pub fn flags(&self) -> u32 {
        u32::from(self.stop_at_entry)
            | u32::from(self.stop_on_library) << 1
            | u32::from(self.stop_on_thread) << 2
            | u32::from(self.disable_aslr) << 3
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DbgRun {
    #[default]
    None,
    Running,
    Suspended,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DbgStateInfo {
    pub state: DbgRun,
    pub pid: u32,
    pub tid: Option<u32>,
    /// Instruction pointer and stack pointer while suspended.
    pub ip: Option<Ea>,
    pub sp: Option<Ea>,
    /// The database's image base before the session (set while a process
    /// lives).
    pub orig_base: Option<Ea>,
    /// The database's image base now.
    pub base: Ea,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgProcess {
    pub pid: u32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgBpt {
    pub ea: Ea,
    pub enabled: bool,
    /// soft, exec, write, read or rw.
    pub kind: String,
    pub size: u32,
    pub cond: String,
    pub lang: String,
    /// Stops at this breakpoint in the current session.
    pub hits: u32,
    pub active: bool,
    pub bad: bool,
    pub symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgReg {
    pub name: String,
    pub bits: u32,
    pub class: String,
    /// Big-endian hex digits, `bits / 4` of them.
    pub hex: String,
    pub float: bool,
    pub ip: bool,
    pub sp: bool,
    pub fp: bool,
    pub address: bool,
    pub readonly: bool,
    /// `(bit, name)` for the named bits of a flags register.
    pub bit_names: Vec<(u32, String)>,
}

impl DbgReg {
    /// The value of an integer register of at most 64 bits.
    pub fn value(&self) -> Option<u64> {
        if self.float || self.bits > 64 {
            return None;
        }
        u64::from_str_radix(&self.hex, 16).ok()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgMem {
    pub ea: Ea,
    /// Two hex digits per byte, `??` where the byte could not be read.
    pub hex: String,
}

impl DbgMem {
    pub fn bytes(&self) -> Vec<Option<u8>> {
        self.hex
            .as_bytes()
            .chunks(2)
            .map(|c| {
                std::str::from_utf8(c)
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgThread {
    pub tid: u32,
    pub name: String,
    pub current: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgModule {
    pub name: String,
    pub base: Ea,
    pub size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgFrame {
    /// The call instruction (the current instruction for frame 0).
    pub ea: Ea,
    pub func: Ea,
    pub fp: Ea,
    pub symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbgException {
    pub code: u32,
    pub name: String,
    pub desc: String,
    /// Suspend when it happens.
    pub stop: bool,
    /// Pass it to the application (otherwise the debugger swallows it).
    pub pass: bool,
}

/// A `{"event":"debug","kind":...}` line.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DebugEvent {
    /// The process state changed. `reason` says why (`breakpoint`, `step`,
    /// `exception`, `continue`, `start`, `exited`, ...); `ip` and `tid` are
    /// set when it is suspended.
    State {
        state: DbgRun,
        ip: Option<Ea>,
        tid: Option<u32>,
        reason: String,
    },
    ProcessStarted {
        pid: u32,
        name: String,
        base: Ea,
        size: u64,
    },
    ProcessAttached {
        pid: u32,
        name: String,
        base: Ea,
        size: u64,
    },
    ProcessExited {
        pid: u32,
        exit_code: i32,
    },
    ProcessDetached {
        pid: u32,
    },
    ThreadStarted {
        tid: u32,
        name: String,
    },
    ThreadExited {
        tid: u32,
        exit_code: i32,
    },
    LibLoaded {
        name: String,
        base: Ea,
        size: u64,
    },
    LibUnloaded {
        name: String,
    },
    /// A stop at a breakpoint (also reported as `state`).
    Breakpoint {
        tid: u32,
        ea: Ea,
        hits: u32,
    },
    Step {
        tid: u32,
        ea: Ea,
    },
    Exception {
        tid: u32,
        ea: Ea,
        code: u32,
        name: String,
        desc: String,
        info: String,
        can_continue: bool,
        handled: bool,
        stopped: bool,
        /// False when the same exception repeats at the same address after
        /// a continue (the last chance). IDA does not report the chance, so
        /// this is inferred.
        first_chance: bool,
    },
    /// Text the debuggee sent with `OutputDebugString`.
    Output {
        tid: u32,
        text: String,
    },
    /// The kernel moved the database base (process start, or restored when
    /// the process is gone). Every address the UI holds shifts by `delta`.
    Rebased {
        old_base: Ea,
        base: Ea,
        delta: i64,
        restored: bool,
    },
    /// A pause request did not complete in time.
    PauseTimeout,
    Error {
        text: String,
    },
}
