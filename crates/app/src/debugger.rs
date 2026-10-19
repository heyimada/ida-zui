// UI-side debugger state: what the host last said about the process, the
// breakpoints, registers, stack and so on, and the pure helpers the panels
// use to turn that into rows. `session_debug.rs` fills it from host events
// and requests; the views only read it.
use ida_proto::{DbgBpt, DbgException, DbgFrame, DbgMem, DbgModule, DbgReg, DbgRun, DbgThread, Ea};
use std::collections::{HashMap, HashSet};

/// Most lines kept in the events log.
const LOG_LIMIT: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogKind {
    Info,
    /// Text the debuggee printed with `OutputDebugString`.
    Output,
    Warn,
    Error,
}

pub struct LogLine {
    pub kind: LogKind,
    pub text: String,
}

/// An exception the process stopped on.
#[derive(Clone, Debug)]
pub struct StopException {
    pub code: u32,
    pub name: String,
    pub first_chance: bool,
}

/// One word of the stack panel.
#[derive(Clone, Debug, PartialEq)]
pub struct StackWord {
    pub addr: Ea,
    /// None when the word could not be read.
    pub value: Option<u64>,
    /// `name+0xN` for a value that points into the program or a module.
    pub sym: Option<String>,
}

/// One row of a memory dump.
#[derive(Clone, Debug, PartialEq)]
pub struct DumpRow {
    pub addr: Ea,
    pub bytes: Vec<Option<u8>>,
    pub ascii: String,
}

#[derive(Default)]
pub struct DebugModel {
    pub state: DbgRun,
    pub pid: u32,
    pub tid: Option<u32>,
    pub ip: Option<Ea>,
    pub sp: Option<Ea>,
    /// Why the process last stopped (`breakpoint`, `step`, `exception`, ...).
    pub reason: String,
    pub exception: Option<StopException>,
    pub bpts: Vec<DbgBpt>,
    pub regs: Vec<DbgReg>,
    /// Register values at the stop before this one, for highlighting.
    pub prev_regs: HashMap<String, String>,
    pub changed: HashSet<String>,
    pub threads: Vec<DbgThread>,
    pub modules: Vec<DbgModule>,
    pub frames: Vec<DbgFrame>,
    pub stack: Vec<StackWord>,
    /// The expression the memory panel follows, its value and the bytes.
    pub mem_expr: String,
    pub mem_addr: Option<Ea>,
    pub mem: Option<DbgMem>,
    pub exceptions: Vec<DbgException>,
    pub log: Vec<LogLine>,
    /// Bumped on every change a view may want to repaint for.
    pub revision: u64,
}

impl DebugModel {
    pub fn new() -> Self {
        Self {
            mem_expr: "RIP".into(),
            ..Self::default()
        }
    }

    /// A process exists (running or suspended).
    pub fn active(&self) -> bool {
        self.state != DbgRun::None
    }

    pub fn suspended(&self) -> bool {
        self.state == DbgRun::Suspended
    }

    pub fn bpt_at(&self, ea: Ea) -> Option<&DbgBpt> {
        self.bpts.iter().find(|b| b.ea == ea)
    }

    pub fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        self.log.push(LogLine {
            kind,
            text: text.into(),
        });
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..LOG_LIMIT / 4);
        }
    }

    /// The process is gone: keep the breakpoints, drop everything else.
    pub fn clear_process(&mut self) {
        self.state = DbgRun::None;
        self.pid = 0;
        self.tid = None;
        self.ip = None;
        self.sp = None;
        self.exception = None;
        self.regs.clear();
        self.prev_regs.clear();
        self.changed.clear();
        self.threads.clear();
        self.modules.clear();
        self.frames.clear();
        self.stack.clear();
        self.mem = None;
        self.mem_addr = None;
    }

    /// The instruction pointer register's value.
    pub fn reg_ip(&self) -> Option<Ea> {
        self.regs.iter().find(|r| r.ip)?.value()
    }

    /// The register panel's rows: remember the previous values, then mark
    /// what changed.
    pub fn set_regs(&mut self, regs: Vec<DbgReg>) {
        self.prev_regs = self
            .regs
            .iter()
            .map(|r| (r.name.clone(), r.hex.clone()))
            .collect();
        self.changed = changed_regs(&self.prev_regs, &regs);
        self.regs = regs;
    }
}

/// `ea` moved by `delta`, stopping at zero instead of wrapping.
pub fn shift(ea: Ea, delta: i64) -> Ea {
    if delta >= 0 {
        ea.wrapping_add(delta as u64)
    } else {
        ea.saturating_sub(delta.unsigned_abs())
    }
}

/// Registers whose value differs from the snapshot `prev`. A register the
/// snapshot does not know is not a change: an empty snapshot (the first
/// stop) highlights nothing.
pub fn changed_regs(prev: &HashMap<String, String>, regs: &[DbgReg]) -> HashSet<String> {
    regs.iter()
        .filter(|r| prev.get(&r.name).is_some_and(|old| *old != r.hex))
        .map(|r| r.name.clone())
        .collect()
}

/// The single-bit flags of a flags register with their state, in bit order.
/// A name that covers several bits (IOPL) is a field, not a flag.
pub fn flag_states(bit_names: &[(u32, String)], value: u64) -> Vec<(String, bool)> {
    bit_names
        .iter()
        .filter(|(_, n)| bit_names.iter().filter(|(_, m)| m == n).count() == 1)
        .map(|(bit, n)| (n.clone(), bit < &64 && value >> bit & 1 == 1))
        .collect()
}

/// Splits bytes read from `base` into rows of `per_row`; an unreadable byte
/// is `None` and shows as `?` in the text column.
pub fn dump_rows(base: Ea, bytes: &[Option<u8>], per_row: usize) -> Vec<DumpRow> {
    bytes
        .chunks(per_row.max(1))
        .enumerate()
        .map(|(i, chunk)| DumpRow {
            addr: base + (i * per_row) as u64,
            bytes: chunk.to_vec(),
            ascii: chunk
                .iter()
                .map(|b| match b {
                    None => '?',
                    Some(c) if (0x20..0x7f).contains(c) => *c as char,
                    Some(_) => '.',
                })
                .collect(),
        })
        .collect()
}

/// Little-endian words of `word` bytes read from `base`.
pub fn stack_words(base: Ea, bytes: &[Option<u8>], word: usize) -> Vec<StackWord> {
    bytes
        .chunks(word.max(1))
        .enumerate()
        .map(|(i, chunk)| {
            let value = (chunk.len() == word && chunk.iter().all(Option::is_some)).then(|| {
                chunk
                    .iter()
                    .rev()
                    .fold(0u64, |acc, b| acc << 8 | u64::from(b.unwrap_or(0)))
            });
            StackWord {
                addr: base + (i * word) as u64,
                value,
                sym: None,
            }
        })
        .collect()
}

/// The pseudocode line the instruction pointer is in: the line holding the
/// closest item address at or before `ip` (the earliest line on a tie).
pub fn ip_line(line_eas: &[Vec<Ea>], ip: Ea) -> Option<usize> {
    let mut best: Option<(Ea, usize)> = None;
    for (i, eas) in line_eas.iter().enumerate() {
        if let Some(ea) = eas.iter().copied().filter(|e| *e <= ip).max()
            && best.is_none_or(|(b, _)| ea > b)
        {
            best = Some((ea, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Where a breakpoint set on a pseudocode line goes: its lowest address.
pub fn line_bpt_ea(eas: &[Ea]) -> Option<Ea> {
    eas.iter().copied().min()
}

#[cfg(test)]
mod tests;
