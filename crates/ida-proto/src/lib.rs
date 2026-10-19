//! Protocol between the UI and `ida-host.exe`.
//!
//! The host owns idalib on its main thread. The UI sends newline-delimited
//! JSON requests `{"id","method","params"}` on the host's stdin and receives
//! `{"id","ok"}` / `{"id","err"}` lines on stdout. The host also writes
//! unsolicited `{"event": ...}` lines (no `id`): IDA's messages, coalesced
//! database changes and analysis progress, see [`HostEvent`]. Anything IDA
//! prints outside its message hook goes to the host's stderr; the client
//! surfaces both as log lines.

pub mod client;
pub mod debug;
pub mod declerr;
pub mod tags;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub use client::{Client, ClientError};
pub use debug::*;

pub type Ea = u64;
pub const BADADDR: Ea = u64::MAX;

/// A request type and its reply.
pub trait Method: Serialize + DeserializeOwned + Send + 'static {
    const NAME: &'static str;
    type Reply: Serialize + DeserializeOwned + Send + 'static;
}

#[derive(Serialize, Deserialize)]
pub struct Request<'a> {
    pub id: u64,
    pub method: &'a str,
    pub params: Box<serde_json::value::RawValue>,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<Box<serde_json::value::RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
}

macro_rules! methods {
    ($(
        $(#[$meta:meta])*
        $ty:ident = $wire:literal { $($field:ident : $fty:ty),* $(,)? } -> $reply:ty;
    )*) => {
        $(
            $(#[$meta])*
            #[derive(Clone, Debug, Serialize, Deserialize)]
            pub struct $ty { $(pub $field: $fty),* }
            impl Method for $ty {
                const NAME: &'static str = $wire;
                type Reply = $reply;
            }
        )*
        pub const METHOD_NAMES: &[&str] = &[$($wire),*];
    };
}

methods! {
    /// Opens (and optionally auto-analyses) a binary or an existing database.
    /// With `base`, the program is rebased to that address before analysis.
    /// Loader and processor choices are made when the host is started, see
    /// `client::HostConfig`.
    Open = "open" { path: String, auto_analyze: bool, base: Option<Ea> } -> DbInfo;
    /// IDA and host versions and paths; works without an open database.
    Version = "version" {} -> VersionInfo;
    Info = "info" {} -> DbInfo;
    Save = "save" {} -> ();
    /// Columns of the instruction and the comment in generated listing lines
    /// (IDA's defaults are 16 and 40). Stored in the database when saved.
    SetListingOptions = "set_listing_options" { indent: u32, cmt_indent: u32 } -> ();
    /// Closes the database; the host exits afterwards.
    Close = "close" { save: bool } -> ();

    Functions = "functions" {} -> Vec<FuncRow>;
    Names = "names" {} -> Vec<NameRow>;
    Strings = "strings" {} -> Vec<StrRow>;
    Imports = "imports" {} -> Vec<ImportRow>;
    Exports = "exports" {} -> Vec<ExportRow>;
    Segments = "segments" {} -> Vec<SegRow>;
    LocalTypes = "local_types" {} -> Vec<TypeRow>;
    /// Local types without declarations: cheap on databases with thousands.
    TypeList = "type_list" {} -> Vec<TypeBrief>;
    /// One type's full multi-line C declaration.
    TypeDecl = "type_decl" { name: String } -> TypeDeclReply;
    /// Parses any number of declarations into the local types, replacing
    /// types of the same name. A statement with errors stores nothing; the
    /// others stay. Parser errors come back as data in the reply.
    TypeApply = "type_apply" { decl: String } -> ApplyReply;
    /// Creates an empty `struct`, `union` or `enum`.
    TypeCreate = "type_create" { name: String, kind: String } -> ();
    /// Types that refer to the one being deleted block it unless `force`.
    TypeDelete = "type_delete" { name: String, force: bool } -> DeleteReply;

    UdtInfo = "udt_info" { name: String } -> UdtInfoReply;
    /// Adds a member at byte `offset` (the end when `None`). `decl` is a type
    /// such as `int`, `char [16]` or `void *`.
    UdtAdd = "udt_add" { name: String, member: String, decl: String, offset: Option<u64> } -> ();
    UdtRename = "udt_rename" { name: String, index: u32, new: String } -> ();
    /// With `force`, members the new type overlaps are destroyed.
    UdtRetype = "udt_retype" { name: String, index: u32, decl: String, force: bool } -> ();
    UdtDelete = "udt_delete" { name: String, index: u32 } -> ();
    UdtComment = "udt_comment" { name: String, index: u32, text: String } -> ();
    UdtXrefs = "udt_xrefs" { name: String, index: u32 } -> Vec<XrefRow>;

    EnumInfo = "enum_info" { name: String } -> EnumInfoReply;
    EnumAdd = "enum_add" { name: String, member: String, value: u64 } -> ();
    EnumRename = "enum_rename" { name: String, index: u32, new: String } -> ();
    EnumSetValue = "enum_set_value" { name: String, index: u32, value: u64 } -> ();
    EnumDelete = "enum_delete" { name: String, index: u32 } -> ();
    EnumComment = "enum_comment" { name: String, index: u32, text: String } -> ();
    /// Switches between an ordinary enum and a bit-flag enum.
    EnumSetBitfield = "enum_set_bitfield" { name: String, on: bool } -> ();

    /// The stack frame of the function containing `func`. Offsets are into
    /// the frame structure (lowest address first).
    FrameInfo = "frame_info" { func: Ea } -> FrameInfoReply;
    FrameDefine = "frame_define" { func: Ea, offset: u64, name: String, decl: String } -> ();
    FrameRename = "frame_rename" { func: Ea, offset: u64, new: String } -> ();
    FrameRetype = "frame_retype" { func: Ea, offset: u64, decl: String, force: bool } -> ();
    FrameDelete = "frame_delete" { func: Ea, offset: u64 } -> ();
    FrameComment = "frame_comment" { func: Ea, offset: u64, text: String } -> ();
    FrameXrefs = "frame_xrefs" { func: Ea, offset: u64 } -> Vec<XrefRow>;

    /// Listing lines for items starting at the item containing `ea`, until at
    /// least `count` lines are produced.
    ListingDown = "listing_down" { ea: Ea, count: u32 } -> ListingChunk;
    /// Listing lines for the items strictly before `ea`, in address order.
    ListingUp = "listing_up" { ea: Ea, count: u32 } -> ListingChunk;
    /// Control-flow graph of the function containing `ea`.
    Graph = "graph" { ea: Ea } -> FlowGraph;
    Decompile = "decompile" { ea: Ea } -> Pseudocode;
    /// What a pseudocode anchor (ctree item index) refers to.
    PseudoItem = "pseudo_item" { func: Ea, item: u64 } -> ItemInfo;
    RenameLvar = "rename_lvar" { func: Ea, old: String, new: String } -> ();
    SetLvarType = "set_lvar_type" { func: Ea, name: String, decl: String } -> ();

    AddrInfo = "addr_info" { ea: Ea } -> AddrInfoReply;
    /// Facts about the function containing `ea` for the inspector; `None`
    /// when `ea` is not in a function.
    FuncSummary = "func_summary" { ea: Ea } -> Option<FuncSummaryReply>;
    /// Resolves an address expression or name (as IDA's "Jump to address").
    Resolve = "resolve" { expr: String, from: Ea } -> Option<Ea>;
    XrefsTo = "xrefs_to" { ea: Ea } -> Vec<XrefRow>;
    XrefsFrom = "xrefs_from" { ea: Ea } -> Vec<XrefRow>;
    Bytes = "bytes" { ea: Ea, len: u32 } -> ByteChunk;
    FindBytes = "find_bytes" { ea: Ea, pattern: String, forward: bool } -> Option<Ea>;
    FindText = "find_text" { ea: Ea, text: String, forward: bool } -> Option<Ea>;

    Rename = "rename" { ea: Ea, name: String } -> ();
    SetComment = "set_comment" { ea: Ea, text: String, repeatable: bool } -> ();
    SetFuncComment = "set_func_comment" { ea: Ea, text: String, repeatable: bool } -> ();
    GetComment = "get_comment" { ea: Ea, repeatable: bool } -> String;
    GetType = "get_type" { ea: Ea } -> String;
    SetType = "set_type" { ea: Ea, decl: String } -> ();
    SetLocalType = "set_local_type" { decl: String } -> ();
    MakeCode = "make_code" { ea: Ea } -> ();
    /// Cycles byte → word → dword → qword like IDA's `D`.
    MakeData = "make_data" { ea: Ea } -> ();
    MakeString = "make_string" { ea: Ea } -> ();
    Undefine = "undefine" { ea: Ea } -> ();
    MakeFunction = "make_function" { ea: Ea } -> ();
    DeleteFunction = "delete_function" { ea: Ea } -> ();
    /// Expands or collapses the function or hidden range at `ea`.
    ToggleCollapse = "toggle_collapse" { ea: Ea } -> ();
    Undo = "undo" {} -> bool;
    Redo = "redo" {} -> bool;

    /// Runs IDAPython source like IDA's console: statements run, and a
    /// trailing expression's value is printed. Unless `force`, source that is
    /// not a complete statement yet (an open block, bracket) runs nothing and
    /// comes back with `incomplete`. `ea` is what `idc.here()` reports.
    PyEval = "py_eval" { source: String, ea: Ea, force: bool } -> PyReply;
    /// Runs a Python script file (File > Script file).
    PyRunFile = "py_run_file" { path: String, ea: Ea } -> PyReply;
    /// Completions of the dotted name `word` in the console's namespace, from
    /// Python's `rlcompleter` (sorted, at most 200).
    PyComplete = "py_complete" { word: String } -> Vec<String>;
    /// Raises KeyboardInterrupt in the running script. Answered at once, even
    /// while another request is executing.
    Interrupt = "interrupt" {} -> ();
    /// Asks the running auto-analysis to stop between two steps (the remaining
    /// queue is dropped and `open` / the request in progress returns).
    /// Answered at once, like `interrupt`.
    AnalysisStop = "analysis_stop" {} -> ();

    /// FLIRT: the planned and applied signatures, the `.sig` files available
    /// for the processor, and how many library functions the database has.
    SigList = "sig_list" {} -> SigState;
    /// Plans the named signature files (names without `.sig`) and runs the
    /// analysis queues over the database, with `analysis` progress events.
    SigApply = "sig_apply" { names: Vec<String> } -> SigApplyReply;
    /// The configured Lumina server (read from IDA's settings; never changed).
    LuminaStatus = "lumina_status" {} -> LuminaStatusReply;
    /// Runs one of the kernel's Lumina actions. A push uploads the analysis to
    /// the server, so the host refuses it unless `confirmed` (and always under
    /// automation); `ea` picks the function of the single-function actions.
    Lumina = "lumina" { action: LuminaAction, ea: Ea, confirmed: bool } -> LuminaReply;
    /// The call neighbourhood of the function at `ea`: callers up to `up`
    /// levels, callees down to `down` levels, at most `fanout` neighbours per
    /// node and side and `max_nodes` nodes. `data` adds referenced data.
    /// The loader's plugin list (from every plugins directory on IDAUSR and
    /// IDADIR). Listing loads nothing.
    PluginList = "plugin_list" {} -> Vec<PluginInfo>;
    /// Runs a plugin chosen by its `path` (or short name) with `arg`, loading
    /// it first when needed. idalib has no Qt UI: a plugin that needs one does
    /// nothing, which the reply tries to tell apart from a silent success.
    PluginRun = "plugin_run" { plugin: String, arg: u64 } -> PluginRunReply;
    Proximity = "proximity" { ea: Ea, up: u32, down: u32, fanout: u32, max_nodes: u32, data: bool } -> ProximityGraph;

    /// Starts the debuggee (`path` defaults to the input file). The database
    /// is rebased to the runtime image base while the process lives.
    DbgStart = "dbg_start" { path: Option<String>, args: String, cwd: String, options: DbgOptions } -> DbgStateInfo;
    /// Running processes the debugger can attach to.
    DbgProcesses = "dbg_processes" {} -> Vec<DbgProcess>;
    DbgAttach = "dbg_attach" { pid: u32, options: DbgOptions } -> DbgStateInfo;
    DbgDetach = "dbg_detach" {} -> DbgStateInfo;
    DbgTerminate = "dbg_terminate" {} -> DbgStateInfo;
    DbgState = "dbg_state" {} -> DbgStateInfo;
    /// Resumes a suspended process. The call returns at once; the next stop
    /// arrives as `debug` events (`state` with `suspended`).
    DbgResume = "dbg_resume" { how: ResumeHow, ea: Ea } -> DbgStateInfo;
    /// Asks a running process to suspend. A target that never reaches a safe
    /// point yields a `pause_timeout` event after `timeout_ms`.
    DbgPause = "dbg_pause" { timeout_ms: u32 } -> DbgStateInfo;
    DbgBpts = "dbg_bpts" {} -> Vec<DbgBpt>;
    /// Adds a breakpoint (`kind`: soft, exec, write, read, rw). `lang` is
    /// the condition language (IDC or Python), empty for the default.
    DbgBptAdd = "dbg_bpt_add" { ea: Ea, kind: String, size: u32, cond: String, lang: String } -> Vec<DbgBpt>;
    DbgBptDel = "dbg_bpt_del" { ea: Ea } -> Vec<DbgBpt>;
    DbgBptEnable = "dbg_bpt_enable" { ea: Ea, enabled: bool } -> Vec<DbgBpt>;
    DbgBptCond = "dbg_bpt_cond" { ea: Ea, cond: String, lang: String } -> Vec<DbgBpt>;
    /// Registers of the current thread (the general class, or all).
    DbgRegs = "dbg_regs" { all: bool } -> Vec<DbgReg>;
    DbgSetReg = "dbg_set_reg" { name: String, value: u64 } -> ();
    /// Process memory; unreadable bytes read as `??` in `hex`.
    DbgMemRead = "dbg_mem_read" { ea: Ea, len: u32 } -> DbgMem;
    DbgMemWrite = "dbg_mem_write" { ea: Ea, hex: String } -> u32;
    DbgThreads = "dbg_threads" {} -> Vec<DbgThread>;
    DbgSelectThread = "dbg_select_thread" { tid: u32 } -> DbgStateInfo;
    DbgModules = "dbg_modules" {} -> Vec<DbgModule>;
    DbgStack = "dbg_stack" {} -> Vec<DbgFrame>;
    /// `name`, `func+0xN` or `module+0xN` for each address, null if unknown.
    DbgSymbolize = "dbg_symbolize" { eas: Vec<Ea> } -> Vec<Option<String>>;
    /// An IDC expression (registers, names, numbers) as a number.
    DbgEval = "dbg_eval" { expr: String } -> Ea;
    DbgExceptions = "dbg_exceptions" {} -> Vec<DbgException>;
    DbgSetException = "dbg_set_exception" { code: u32, stop: bool, pass: bool } -> Vec<DbgException>;
}

/// `plugin_t::flags` bits, as `PLUGIN_*` in loader.hpp.
pub mod plugin_flags {
    pub const MOD: i32 = 0x0001;
    pub const DRAW: i32 = 0x0002;
    pub const SEG: i32 = 0x0004;
    pub const UNL: i32 = 0x0008;
    pub const HIDE: i32 = 0x0010;
    pub const DBG: i32 = 0x0020;
    pub const PROC: i32 = 0x0040;
    pub const FIX: i32 = 0x0080;
    pub const MULTI: i32 = 0x0100;
    pub const SCRIPTED: i32 = 0x8000;

    /// Short names of the bits set in `flags`, in a stable order.
    pub fn names(flags: i32) -> Vec<&'static str> {
        const ALL: [(i32, &str); 10] = [
            (MOD, "MOD"),
            (DRAW, "DRAW"),
            (SEG, "SEG"),
            (UNL, "UNL"),
            (HIDE, "HIDE"),
            (DBG, "DBG"),
            (PROC, "PROC"),
            (FIX, "FIX"),
            (MULTI, "MULTI"),
            (SCRIPTED, "SCRIPTED"),
        ];
        ALL.iter()
            .filter(|(bit, _)| flags & bit != 0)
            .map(|(_, n)| *n)
            .collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginInfo {
    /// The name shown in IDA's Plugins menu.
    pub name: String,
    /// The name the plugin asked for.
    pub org_name: String,
    /// File base name (or `ida-plugin.json` name).
    pub id: String,
    pub path: String,
    /// The plugin's own wanted hotkey ("Ctrl-Alt-F"); empty until it is loaded.
    pub hotkey: String,
    pub hotkey_code: u64,
    /// The argument this entry runs with (a plugin can be listed once per argument).
    pub arg: u64,
    pub flags: i32,
    pub loaded: bool,
    pub comment: String,
    #[serde(default)]
    pub help: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginRunReply {
    pub name: String,
    pub path: String,
    pub was_loaded: bool,
    /// IDA could load the plugin's module and get its descriptor.
    pub loadable: bool,
    /// `run` returned true (Python plugins usually return nothing, so false
    /// does not mean failure).
    pub returned: bool,
    /// The log shows the plugin raised an exception.
    pub failed: bool,
    /// The plugin changed the database.
    pub changed: bool,
    /// The log mentions a Qt or widget problem.
    pub needs_gui: bool,
    pub log: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SigApplied {
    pub index: u32,
    pub name: String,
    pub title: String,
    pub optlibs: String,
    /// `applied`, `current` or `planned`.
    pub state: String,
    /// Modules this signature recognised.
    pub matched: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SigAvailable {
    pub name: String,
    pub title: String,
    pub path: String,
    /// Functions the signature file describes.
    pub functions: u64,
    pub applied: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SigState {
    pub processor: String,
    /// Functions flagged as library functions.
    pub lib_functions: u64,
    pub applied: Vec<SigApplied>,
    pub available: Vec<SigAvailable>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SigApplyReply {
    /// Signatures IDA accepted into its list.
    pub planned: u64,
    /// Names IDA could not find or load.
    pub failed: Vec<String>,
    pub lib_before: u64,
    pub lib_after: u64,
    pub state: SigState,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LuminaStatusReply {
    /// `user@host:port` as set in IDA's options.
    pub server: String,
    pub secondary: String,
    pub auto_use: bool,
    /// Functions whose metadata came from Lumina.
    pub functions: u64,
    /// Pushes are refused in this host (automation).
    pub push_blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LuminaAction {
    PullAll,
    PullFunc,
    PushAll,
    PushFunc,
}

impl LuminaAction {
    /// Uploads the user's analysis.
    pub fn is_push(self) -> bool {
        matches!(self, LuminaAction::PushAll | LuminaAction::PushFunc)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LuminaReply {
    /// The kernel action that ran, e.g. `LuminaPullAllMds`.
    pub action: String,
    /// The kernel accepted the action. It says nothing about the result: read
    /// `log` and the counts.
    pub dispatched: bool,
    pub functions: u64,
    /// Functions with Lumina metadata before and after.
    pub lumina_before: u64,
    pub lumina_after: u64,
    /// What IDA printed while the action ran.
    pub log: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProximityNode {
    pub ea: Ea,
    pub name: String,
    /// `func`, `import` or `data`.
    pub kind: String,
    /// Levels from the centre: negative for callers, positive for callees.
    pub depth: i32,
    /// Distinct callers and callees in the database (not only those shown).
    pub callers: u64,
    pub callees: u64,
    pub library: bool,
    pub thunk: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ProximityEdge {
    pub from: Ea,
    pub to: Ea,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProximityGraph {
    pub centre: Ea,
    /// `max_nodes` cut the neighbourhood short.
    pub truncated: bool,
    pub nodes: Vec<ProximityNode>,
    pub edges: Vec<ProximityEdge>,
}

/// A run of output from a script: `out` (stdout), `err` (stderr), `result`
/// (the printed value of a trailing expression) or `tb` (a traceback).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PyChunk(pub String, pub String);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PyReply {
    /// The source is not a complete statement; nothing ran.
    pub incomplete: bool,
    /// Output in the order it was produced.
    pub chunks: Vec<PyChunk>,
    /// The traceback when the script raised.
    pub error: Option<String>,
    /// The script changed the database.
    pub changed: bool,
    pub elapsed_ms: u64,
}

/// Where a database change came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOrigin {
    /// An ordinary request, such as the UI's own rename.
    Request,
    /// A script running in the host.
    Python,
}

/// Database changes since the last event, coalesced: at most about 8 per
/// second however many edits a script makes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbChanged {
    pub origin: ChangeOrigin,
    /// Any of `name`, `cmt`, `func`, `type`, `code`, `data`, `patch`, `seg`,
    /// `ltype`, `frame`, `display`.
    pub kinds: Vec<String>,
    /// Some change has no address (segments, local types): refetch everything.
    pub global: bool,
    /// Smallest and largest address touched, when addresses are known.
    pub lo: Option<Ea>,
    pub hi: Option<Ea>,
    /// How many notifications were folded into this event.
    pub count: u64,
}

impl DbChanged {
    /// Names, functions or segments changed, which the side lists show.
    pub fn affects_lists(&self) -> bool {
        self.global
            || self
                .kinds
                .iter()
                .any(|k| matches!(k.as_str(), "name" | "func" | "seg"))
    }
}

/// Progress of the auto-analysis queues.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisEvent {
    /// The queue being worked (`AU_CODE`, `AU_PROC`, ...), `AU_NONE` at the
    /// start and `done` at the end.
    pub stage: String,
    pub ea: Option<Ea>,
    /// An estimate that never goes down; 100 only with `done`.
    pub percent: u32,
    /// Functions found so far.
    pub functions: u64,
    pub elapsed_ms: u64,
    pub done: bool,
    /// The first event of a run.
    pub start: bool,
}

impl AnalysisEvent {
    /// What the queue in `stage` is doing, for people.
    pub fn stage_label(&self) -> &'static str {
        if self.done {
            return "Done";
        }
        match self.stage.as_str() {
            "AU_UNK" => "Converting to unexplored bytes",
            "AU_CODE" | "AU_WEAK" => "Finding code",
            "AU_PROC" | "AU_TAIL" | "AU_FCHUNK" => "Creating functions",
            "AU_USED" | "AU_USD2" => "Analyzing instructions",
            "AU_TYPE" => "Applying types",
            "AU_LIBF" | "AU_LBF2" | "AU_LBF3" | "AU_CHLB" => "Matching signatures",
            "AU_FINAL" => "Final pass",
            _ => "Starting analysis",
        }
    }
}

/// Unsolicited lines from the host: `{"event": ..., ...}` with no `id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HostEvent {
    /// Text IDA printed (also delivered through the client's log channel).
    Log {
        lines: Vec<String>,
    },
    DbChanged(DbChanged),
    Analysis(AnalysisEvent),
    Debug(DebugEvent),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct VersionInfo {
    /// IDA's version string, e.g. `9.x.0`.
    pub ida_version: String,
    pub ida_dir: String,
    /// IDA's per-user directory (plugins, cfg).
    pub user_dir: String,
    /// The SDK the host was built against.
    pub sdk_dir: String,
    pub host_exe: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DbInfo {
    pub input_path: String,
    pub idb_path: String,
    pub processor: String,
    pub bitness: u32,
    pub min_ea: Ea,
    pub max_ea: Ea,
    /// The image base (moves while a process is being debugged).
    #[serde(default)]
    pub image_base: Ea,
    pub entry: Option<Ea>,
    pub hexrays: bool,
    pub file_type: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FuncRow {
    pub start: Ea,
    pub end: Ea,
    pub name: String,
    pub library: bool,
    pub thunk: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NameRow {
    pub ea: Ea,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrRow {
    pub ea: Ea,
    pub len: u32,
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportRow {
    pub ea: Ea,
    pub module: String,
    pub name: String,
    pub ordinal: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportRow {
    pub ea: Ea,
    pub ordinal: u64,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegRow {
    pub start: Ea,
    pub end: Ea,
    pub name: String,
    pub class: String,
    pub perm: String,
    pub bitness: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeRow {
    pub ordinal: u32,
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    /// Full C declaration (multi-line for structs/enums).
    pub decl: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeBrief {
    pub ordinal: u32,
    pub name: String,
    /// `struct`, `union`, `enum`, `func`, `typedef` or `other`.
    pub kind: String,
    pub size: Option<u64>,
    /// Members of a struct, union or enum.
    pub members: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TypeDeclReply {
    pub name: String,
    pub ordinal: u32,
    pub kind: String,
    pub size: Option<u64>,
    pub decl: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApplyReply {
    pub applied: bool,
    /// Error count from the IDA type parser.
    pub errors: u32,
    /// The parser's message text.
    pub messages: String,
    /// Types created by the declarations.
    pub names: Vec<String>,
}

impl ApplyReply {
    /// The parser messages with the line each belongs to in `decl`.
    pub fn errors(&self, decl: &str) -> Vec<declerr::DeclError> {
        declerr::locate(decl, &self.messages)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DeleteReply {
    pub deleted: bool,
    /// Types that refer to the deleted one.
    pub dependents: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UdtMember {
    pub index: u32,
    /// Byte offset of the member (of the storage unit for a bitfield).
    pub offset: u64,
    pub size: u64,
    /// Width in bits, for a bitfield.
    pub bits: Option<u64>,
    pub bit_offset: Option<u64>,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub cmt: String,
    /// An alignment gap the type system inserted.
    pub gap: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UdtInfoReply {
    pub name: String,
    pub ordinal: u32,
    pub is_union: bool,
    pub size: u64,
    pub members: Vec<UdtMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnumMember {
    pub index: u32,
    pub name: String,
    pub value: u64,
    pub cmt: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EnumInfoReply {
    pub name: String,
    pub ordinal: u32,
    /// A bit-flag enum.
    pub bitfield: bool,
    /// Size of the enum's storage in bytes.
    pub width: u64,
    pub radix: u32,
    pub members: Vec<EnumMember>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct FramePart {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameMember {
    pub index: u32,
    /// Offset in the frame structure.
    pub offset: u64,
    /// IDA's frame-pointer relative offset (negative for locals).
    pub fp_off: i64,
    pub size: u64,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub cmt: String,
    /// `local`, `saved_regs`, `return_address` or `arg`.
    pub kind: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FrameInfoReply {
    pub func: Ea,
    pub name: String,
    pub size: u64,
    pub locals_size: u64,
    pub saved_regs_size: u64,
    pub ret_size: u64,
    pub args_size: u64,
    pub fpd: i64,
    pub lvars: FramePart,
    pub saved_regs: FramePart,
    pub return_address: FramePart,
    pub args: FramePart,
    pub members: Vec<FrameMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ListingLine {
    /// Head of the item this line belongs to.
    pub ea: Ea,
    /// Color-tagged text; parse with [`tags::parse`].
    pub tagged: String,
    /// Segment-relative prefix, e.g. `.text:0000000140001010`.
    pub prefix: String,
    /// Line prefix color (see `COLOR_PFX` in lines.hpp).
    pub prefix_color: u8,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ListingChunk {
    pub lines: Vec<ListingLine>,
    /// First item head after the chunk (down) or of the chunk (up).
    pub next: Option<Ea>,
    /// True if the chunk reached the start (up) or end (down) of the database.
    pub at_edge: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlowBlock {
    pub start: Ea,
    pub end: Ea,
    pub lines: Vec<ListingLine>,
    pub succs: Vec<FlowEdge>,
    pub is_ret: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Unconditional transfer or the only successor.
    Always,
    /// Branch taken (jump target of a conditional).
    Taken,
    /// Fall-through of a conditional.
    NotTaken,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FlowEdge {
    pub to: u32,
    pub kind: EdgeKind,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FlowGraph {
    pub func: Ea,
    pub name: String,
    pub blocks: Vec<FlowBlock>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pseudocode {
    pub func: Ea,
    pub name: String,
    pub lines: Vec<String>,
    /// Per line, the addresses of the ctree items on it (empty for lines
    /// that carry none), for markers from the debugger.
    #[serde(default)]
    pub line_eas: Vec<Vec<Ea>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ItemInfo {
    /// Address the item was generated from (for disassembly sync).
    pub ea: Option<Ea>,
    /// `cot_*`/`cit_*` op name without prefix, e.g. `obj`, `var`, `call`.
    pub op: String,
    /// Global object address for `obj` items.
    pub target: Option<Ea>,
    /// Local variable name for `var` items.
    pub lvar: Option<String>,
    pub lvar_type: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AddrInfoReply {
    pub ea: Ea,
    pub head: Ea,
    pub segment: Option<String>,
    pub function: Option<String>,
    pub func_start: Option<Ea>,
    pub name: Option<String>,
    pub kind: String,
    pub file_offset: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CalleeRow {
    pub ea: Ea,
    pub name: String,
    /// An import or thunk rather than code in the database.
    pub imported: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FuncSummaryReply {
    pub start: Ea,
    pub end: Ea,
    pub name: String,
    pub segment: String,
    pub seg_class: String,
    /// `R-X` style permissions of the segment.
    pub seg_perm: String,
    /// `__fastcall`, `__stdcall`, ...
    pub convention: String,
    /// One-line prototype, empty when none is known.
    pub prototype: String,
    pub blocks: u64,
    pub edges: u64,
    pub locals: u64,
    pub frame_size: u64,
    /// Subset of `lib`, `thunk`, `noreturn`, `static`.
    pub flags: Vec<String>,
    pub callees: Vec<CalleeRow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XrefRow {
    pub from: Ea,
    pub to: Ea,
    /// IDA's one-letter xref kind: p(call) j(ump) ^(flow) r w o(ffset) i(nfo).
    pub kind: String,
    pub func: Option<String>,
    /// Plain text of the line at `from` (xrefs_to) or `to` (xrefs_from).
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ByteChunk {
    pub ea: Ea,
    /// Hex pairs; `??` for bytes with no value in the database.
    pub hex: String,
}

impl ByteChunk {
    pub fn get(&self, i: usize) -> Option<u8> {
        let s = self.hex.get(i * 2..i * 2 + 2)?;
        u8::from_str_radix(s, 16).ok()
    }

    pub fn len(&self) -> usize {
        self.hex.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.hex.is_empty()
    }
}
