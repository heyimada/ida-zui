//! `ida-host`: owns idalib on its main thread and serves ida-proto requests
//! read from stdin. Exits on stdin EOF (discarding unsaved changes) or after
//! a `close` request.

use ida_proto::*;
use std::ffi::{CString, c_char, c_void};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;

mod ffi {
    use super::*;
    unsafe extern "C" {
        pub fn izs_take_stdio(proto_in: *mut *mut c_void, proto_out: *mut *mut c_void) -> i32;
        pub fn izs_result(len: *mut usize) -> *const c_char;
        pub fn izs_init(binary_loader: bool) -> i32;
        pub fn izs_set_emitter(emit: extern "C" fn(*const c_char, usize));
        pub fn izs_begin_request(origin: i32);
        pub fn izs_end_request();
        pub fn izs_interrupt() -> i32;
        pub fn izs_analysis_stop() -> i32;
        pub fn izs_py_complete(word: *const c_char) -> i32;
        pub fn izs_py_eval(source: *const c_char, ea: u64, mode: i32, path: *const c_char) -> i32;
        pub fn izs_open(path: *const c_char, run_auto: bool, base: u64) -> i32;
        pub fn izs_version() -> i32;
        pub fn izs_info() -> i32;
        pub fn izs_save() -> i32;
        pub fn izs_set_listing_options(indent: u32, cmt_indent: u32) -> i32;
        pub fn izs_close(save: bool);
        pub fn izs_functions() -> i32;
        pub fn izs_names() -> i32;
        pub fn izs_strings() -> i32;
        pub fn izs_imports() -> i32;
        pub fn izs_exports() -> i32;
        pub fn izs_segments() -> i32;
        pub fn izs_local_types() -> i32;
        pub fn izs_type_list() -> i32;
        pub fn izs_type_decl(name: *const c_char) -> i32;
        pub fn izs_type_apply(decl: *const c_char) -> i32;
        pub fn izs_type_create(name: *const c_char, kind: *const c_char) -> i32;
        pub fn izs_type_delete(name: *const c_char, force: bool) -> i32;
        pub fn izs_udt_info(name: *const c_char) -> i32;
        pub fn izs_udt_add(
            name: *const c_char,
            member: *const c_char,
            decl: *const c_char,
            offset: u64,
        ) -> i32;
        pub fn izs_udt_rename(name: *const c_char, index: u32, new: *const c_char) -> i32;
        pub fn izs_udt_retype(
            name: *const c_char,
            index: u32,
            decl: *const c_char,
            force: bool,
        ) -> i32;
        pub fn izs_udt_delete(name: *const c_char, index: u32) -> i32;
        pub fn izs_udt_comment(name: *const c_char, index: u32, text: *const c_char) -> i32;
        pub fn izs_udt_xrefs(name: *const c_char, index: u32) -> i32;
        pub fn izs_enum_info(name: *const c_char) -> i32;
        pub fn izs_enum_add(name: *const c_char, member: *const c_char, value: u64) -> i32;
        pub fn izs_enum_rename(name: *const c_char, index: u32, new: *const c_char) -> i32;
        pub fn izs_enum_set_value(name: *const c_char, index: u32, value: u64) -> i32;
        pub fn izs_enum_delete(name: *const c_char, index: u32) -> i32;
        pub fn izs_enum_comment(name: *const c_char, index: u32, text: *const c_char) -> i32;
        pub fn izs_enum_set_bitfield(name: *const c_char, on: bool) -> i32;
        pub fn izs_frame_info(ea: Ea) -> i32;
        pub fn izs_frame_define(
            ea: Ea,
            offset: u64,
            name: *const c_char,
            decl: *const c_char,
        ) -> i32;
        pub fn izs_frame_rename(ea: Ea, offset: u64, new: *const c_char) -> i32;
        pub fn izs_frame_retype(ea: Ea, offset: u64, decl: *const c_char, force: bool) -> i32;
        pub fn izs_frame_delete(ea: Ea, offset: u64) -> i32;
        pub fn izs_frame_comment(ea: Ea, offset: u64, text: *const c_char) -> i32;
        pub fn izs_frame_xrefs(ea: Ea, offset: u64) -> i32;
        pub fn izs_listing_down(ea: Ea, count: u32) -> i32;
        pub fn izs_listing_up(ea: Ea, count: u32) -> i32;
        pub fn izs_graph(ea: Ea) -> i32;
        pub fn izs_decompile(ea: Ea) -> i32;
        pub fn izs_pseudo_item(func: Ea, anchor: u64) -> i32;
        pub fn izs_rename_lvar(func: Ea, old: *const c_char, new: *const c_char) -> i32;
        pub fn izs_set_lvar_type(func: Ea, name: *const c_char, decl: *const c_char) -> i32;
        pub fn izs_addr_info(ea: Ea) -> i32;
        pub fn izs_func_summary(ea: Ea) -> i32;
        pub fn izs_resolve(expr: *const c_char, from: Ea) -> i32;
        pub fn izs_xrefs_to(ea: Ea) -> i32;
        pub fn izs_xrefs_from(ea: Ea) -> i32;
        pub fn izs_bytes(ea: Ea, len: u32) -> i32;
        pub fn izs_find_bytes(ea: Ea, pattern: *const c_char, forward: bool) -> i32;
        pub fn izs_find_text(ea: Ea, text: *const c_char, forward: bool) -> i32;
        pub fn izs_rename(ea: Ea, name: *const c_char) -> i32;
        pub fn izs_set_comment(ea: Ea, text: *const c_char, rpt: bool) -> i32;
        pub fn izs_set_func_comment(ea: Ea, text: *const c_char, rpt: bool) -> i32;
        pub fn izs_get_comment(ea: Ea, rpt: bool) -> i32;
        pub fn izs_get_type(ea: Ea) -> i32;
        pub fn izs_set_type(ea: Ea, decl: *const c_char) -> i32;
        pub fn izs_set_local_type(decl: *const c_char) -> i32;
        pub fn izs_make_code(ea: Ea) -> i32;
        pub fn izs_make_data(ea: Ea) -> i32;
        pub fn izs_make_string(ea: Ea) -> i32;
        pub fn izs_undefine(ea: Ea) -> i32;
        pub fn izs_make_function(ea: Ea) -> i32;
        pub fn izs_delete_function(ea: Ea) -> i32;
        pub fn izs_toggle_collapse(ea: Ea) -> i32;
        pub fn izs_plugin_list() -> i32;
        pub fn izs_plugin_run(which: *const c_char, arg: u64) -> i32;
        pub fn izs_sig_list() -> i32;
        pub fn izs_sig_apply(names: *const c_char) -> i32;
        pub fn izs_lumina_status() -> i32;
        pub fn izs_lumina(action: i32, ea: u64, confirmed: bool) -> i32;
        pub fn izs_proximity(
            ea: u64,
            up: u32,
            down: u32,
            fanout: u32,
            max_nodes: u32,
            data: bool,
        ) -> i32;
        pub fn izs_dbg_running() -> i32;
        pub fn izs_dbg_poll();
        pub fn izs_dbg_start(
            path: *const c_char,
            args: *const c_char,
            cwd: *const c_char,
            flags: u32,
        ) -> i32;
        pub fn izs_dbg_processes() -> i32;
        pub fn izs_dbg_attach(pid: u32, flags: u32) -> i32;
        pub fn izs_dbg_state() -> i32;
        pub fn izs_dbg_detach() -> i32;
        pub fn izs_dbg_terminate() -> i32;
        pub fn izs_dbg_resume(how: i32, ea: u64) -> i32;
        pub fn izs_dbg_pause(timeout_ms: u32) -> i32;
        pub fn izs_dbg_bpts() -> i32;
        pub fn izs_dbg_bpt_add(
            ea: u64,
            kind: *const c_char,
            size: u32,
            cond: *const c_char,
            lang: *const c_char,
        ) -> i32;
        pub fn izs_dbg_bpt_del(ea: u64) -> i32;
        pub fn izs_dbg_bpt_enable(ea: u64, on: bool) -> i32;
        pub fn izs_dbg_bpt_cond(ea: u64, cond: *const c_char, lang: *const c_char) -> i32;
        pub fn izs_dbg_regs(all: bool) -> i32;
        pub fn izs_dbg_set_reg(name: *const c_char, value: u64) -> i32;
        pub fn izs_dbg_mem_read(ea: u64, len: u32) -> i32;
        pub fn izs_dbg_mem_write(ea: u64, hex: *const c_char) -> i32;
        pub fn izs_dbg_threads() -> i32;
        pub fn izs_dbg_select_thread(tid: u32) -> i32;
        pub fn izs_dbg_modules() -> i32;
        pub fn izs_dbg_stack() -> i32;
        pub fn izs_dbg_symbolize(eas: *const c_char) -> i32;
        pub fn izs_dbg_eval(expr: *const c_char) -> i32;
        pub fn izs_dbg_exceptions() -> i32;
        pub fn izs_dbg_set_exception(code: u32, stop: bool, pass: bool) -> i32;
        pub fn izs_undo() -> i32;
        pub fn izs_redo() -> i32;
    }
}

/// The protocol stream. Replies come from the main thread, `interrupt`
/// replies from the reader thread and events from wherever IDA's hooks run;
/// each line is written whole under this lock.
static OUT: std::sync::Mutex<Option<File>> = std::sync::Mutex::new(None);

/// Writes `bytes` and a newline to the protocol stream; false if it is gone.
fn write_line(bytes: &[u8]) -> bool {
    let mut line = Vec::with_capacity(bytes.len() + 1);
    line.extend_from_slice(bytes);
    line.push(b'\n');
    match OUT.lock().unwrap().as_mut() {
        Some(out) => out.write_all(&line).and_then(|()| out.flush()).is_ok(),
        None => false,
    }
}

/// Registered with the shim: one complete event line.
extern "C" fn emit_event(ptr: *const c_char, len: usize) {
    // SAFETY: the shim passes a buffer valid for `len` bytes for this call.
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    write_line(bytes);
}

/// The shim's result buffer after a call returning `rc`.
fn result(rc: i32) -> Result<String, String> {
    let mut len = 0usize;
    // SAFETY: izs_result returns a pointer valid for `len` bytes until the next shim call.
    let text = unsafe {
        let ptr = ffi::izs_result(&mut len);
        String::from_utf8_lossy(std::slice::from_raw_parts(ptr.cast::<u8>(), len)).into_owned()
    };
    if rc == 0 { Ok(text) } else { Err(text) }
}

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "string contains a NUL byte".to_owned())
}

/// Runs one request; returns the reply JSON or an error message.
fn dispatch(method: &str, params: &str) -> Result<String, String> {
    fn p<M: Method>(params: &str) -> Result<M, String> {
        serde_json::from_str(params).map_err(|e| format!("bad params for {}: {e}", M::NAME))
    }
    // SAFETY (all arms): shim calls on the idalib thread with valid C strings.
    unsafe {
        let rc = match method {
            Open::NAME => {
                let m: Open = p(params)?;
                ffi::izs_open(
                    cstr(&m.path)?.as_ptr(),
                    m.auto_analyze,
                    m.base.unwrap_or(BADADDR),
                )
            }
            Version::NAME => {
                // The SDK path is a build-time fact; add it and the host path.
                let json = result(ffi::izs_version())?;
                let mut v: serde_json::Value =
                    serde_json::from_str(&json).map_err(|e| e.to_string())?;
                v["sdk_dir"] = env!("IZS_SDK_DIR").into();
                v["host_exe"] = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
                    .into();
                return Ok(v.to_string());
            }
            Info::NAME => ffi::izs_info(),
            Save::NAME => ffi::izs_save(),
            SetListingOptions::NAME => {
                let m: SetListingOptions = p(params)?;
                ffi::izs_set_listing_options(m.indent, m.cmt_indent)
            }
            Functions::NAME => ffi::izs_functions(),
            Names::NAME => ffi::izs_names(),
            Strings::NAME => ffi::izs_strings(),
            Imports::NAME => ffi::izs_imports(),
            Exports::NAME => ffi::izs_exports(),
            Segments::NAME => ffi::izs_segments(),
            LocalTypes::NAME => ffi::izs_local_types(),
            TypeList::NAME => ffi::izs_type_list(),
            TypeDecl::NAME => ffi::izs_type_decl(cstr(&p::<TypeDecl>(params)?.name)?.as_ptr()),
            TypeApply::NAME => ffi::izs_type_apply(cstr(&p::<TypeApply>(params)?.decl)?.as_ptr()),
            TypeCreate::NAME => {
                let m: TypeCreate = p(params)?;
                ffi::izs_type_create(cstr(&m.name)?.as_ptr(), cstr(&m.kind)?.as_ptr())
            }
            TypeDelete::NAME => {
                let m: TypeDelete = p(params)?;
                ffi::izs_type_delete(cstr(&m.name)?.as_ptr(), m.force)
            }
            UdtInfo::NAME => ffi::izs_udt_info(cstr(&p::<UdtInfo>(params)?.name)?.as_ptr()),
            UdtAdd::NAME => {
                let m: UdtAdd = p(params)?;
                ffi::izs_udt_add(
                    cstr(&m.name)?.as_ptr(),
                    cstr(&m.member)?.as_ptr(),
                    cstr(&m.decl)?.as_ptr(),
                    m.offset.unwrap_or(BADADDR),
                )
            }
            UdtRename::NAME => {
                let m: UdtRename = p(params)?;
                ffi::izs_udt_rename(cstr(&m.name)?.as_ptr(), m.index, cstr(&m.new)?.as_ptr())
            }
            UdtRetype::NAME => {
                let m: UdtRetype = p(params)?;
                ffi::izs_udt_retype(
                    cstr(&m.name)?.as_ptr(),
                    m.index,
                    cstr(&m.decl)?.as_ptr(),
                    m.force,
                )
            }
            UdtDelete::NAME => {
                let m: UdtDelete = p(params)?;
                ffi::izs_udt_delete(cstr(&m.name)?.as_ptr(), m.index)
            }
            UdtComment::NAME => {
                let m: UdtComment = p(params)?;
                ffi::izs_udt_comment(cstr(&m.name)?.as_ptr(), m.index, cstr(&m.text)?.as_ptr())
            }
            UdtXrefs::NAME => {
                let m: UdtXrefs = p(params)?;
                ffi::izs_udt_xrefs(cstr(&m.name)?.as_ptr(), m.index)
            }
            EnumInfo::NAME => ffi::izs_enum_info(cstr(&p::<EnumInfo>(params)?.name)?.as_ptr()),
            EnumAdd::NAME => {
                let m: EnumAdd = p(params)?;
                ffi::izs_enum_add(cstr(&m.name)?.as_ptr(), cstr(&m.member)?.as_ptr(), m.value)
            }
            EnumRename::NAME => {
                let m: EnumRename = p(params)?;
                ffi::izs_enum_rename(cstr(&m.name)?.as_ptr(), m.index, cstr(&m.new)?.as_ptr())
            }
            EnumSetValue::NAME => {
                let m: EnumSetValue = p(params)?;
                ffi::izs_enum_set_value(cstr(&m.name)?.as_ptr(), m.index, m.value)
            }
            EnumDelete::NAME => {
                let m: EnumDelete = p(params)?;
                ffi::izs_enum_delete(cstr(&m.name)?.as_ptr(), m.index)
            }
            EnumComment::NAME => {
                let m: EnumComment = p(params)?;
                ffi::izs_enum_comment(cstr(&m.name)?.as_ptr(), m.index, cstr(&m.text)?.as_ptr())
            }
            EnumSetBitfield::NAME => {
                let m: EnumSetBitfield = p(params)?;
                ffi::izs_enum_set_bitfield(cstr(&m.name)?.as_ptr(), m.on)
            }
            FrameInfo::NAME => ffi::izs_frame_info(p::<FrameInfo>(params)?.func),
            FrameDefine::NAME => {
                let m: FrameDefine = p(params)?;
                ffi::izs_frame_define(
                    m.func,
                    m.offset,
                    cstr(&m.name)?.as_ptr(),
                    cstr(&m.decl)?.as_ptr(),
                )
            }
            FrameRename::NAME => {
                let m: FrameRename = p(params)?;
                ffi::izs_frame_rename(m.func, m.offset, cstr(&m.new)?.as_ptr())
            }
            FrameRetype::NAME => {
                let m: FrameRetype = p(params)?;
                ffi::izs_frame_retype(m.func, m.offset, cstr(&m.decl)?.as_ptr(), m.force)
            }
            FrameDelete::NAME => {
                let m: FrameDelete = p(params)?;
                ffi::izs_frame_delete(m.func, m.offset)
            }
            FrameComment::NAME => {
                let m: FrameComment = p(params)?;
                ffi::izs_frame_comment(m.func, m.offset, cstr(&m.text)?.as_ptr())
            }
            FrameXrefs::NAME => {
                let m: FrameXrefs = p(params)?;
                ffi::izs_frame_xrefs(m.func, m.offset)
            }
            ListingDown::NAME => {
                let m: ListingDown = p(params)?;
                ffi::izs_listing_down(m.ea, m.count)
            }
            ListingUp::NAME => {
                let m: ListingUp = p(params)?;
                ffi::izs_listing_up(m.ea, m.count)
            }
            Graph::NAME => ffi::izs_graph(p::<Graph>(params)?.ea),
            Decompile::NAME => ffi::izs_decompile(p::<Decompile>(params)?.ea),
            PseudoItem::NAME => {
                let m: PseudoItem = p(params)?;
                ffi::izs_pseudo_item(m.func, m.item)
            }
            RenameLvar::NAME => {
                let m: RenameLvar = p(params)?;
                ffi::izs_rename_lvar(m.func, cstr(&m.old)?.as_ptr(), cstr(&m.new)?.as_ptr())
            }
            SetLvarType::NAME => {
                let m: SetLvarType = p(params)?;
                ffi::izs_set_lvar_type(m.func, cstr(&m.name)?.as_ptr(), cstr(&m.decl)?.as_ptr())
            }
            AddrInfo::NAME => ffi::izs_addr_info(p::<AddrInfo>(params)?.ea),
            Resolve::NAME => {
                let m: Resolve = p(params)?;
                ffi::izs_resolve(cstr(&m.expr)?.as_ptr(), m.from)
            }
            FuncSummary::NAME => ffi::izs_func_summary(p::<FuncSummary>(params)?.ea),
            XrefsTo::NAME => ffi::izs_xrefs_to(p::<XrefsTo>(params)?.ea),
            XrefsFrom::NAME => ffi::izs_xrefs_from(p::<XrefsFrom>(params)?.ea),
            Bytes::NAME => {
                let m: Bytes = p(params)?;
                ffi::izs_bytes(m.ea, m.len.min(1 << 20))
            }
            FindBytes::NAME => {
                let m: FindBytes = p(params)?;
                ffi::izs_find_bytes(m.ea, cstr(&m.pattern)?.as_ptr(), m.forward)
            }
            FindText::NAME => {
                let m: FindText = p(params)?;
                ffi::izs_find_text(m.ea, cstr(&m.text)?.as_ptr(), m.forward)
            }
            Rename::NAME => {
                let m: Rename = p(params)?;
                ffi::izs_rename(m.ea, cstr(&m.name)?.as_ptr())
            }
            SetComment::NAME => {
                let m: SetComment = p(params)?;
                ffi::izs_set_comment(m.ea, cstr(&m.text)?.as_ptr(), m.repeatable)
            }
            SetFuncComment::NAME => {
                let m: SetFuncComment = p(params)?;
                ffi::izs_set_func_comment(m.ea, cstr(&m.text)?.as_ptr(), m.repeatable)
            }
            GetComment::NAME => {
                let m: GetComment = p(params)?;
                ffi::izs_get_comment(m.ea, m.repeatable)
            }
            GetType::NAME => ffi::izs_get_type(p::<GetType>(params)?.ea),
            SetType::NAME => {
                let m: SetType = p(params)?;
                ffi::izs_set_type(m.ea, cstr(&m.decl)?.as_ptr())
            }
            SetLocalType::NAME => {
                let m: SetLocalType = p(params)?;
                ffi::izs_set_local_type(cstr(&m.decl)?.as_ptr())
            }
            MakeCode::NAME => ffi::izs_make_code(p::<MakeCode>(params)?.ea),
            MakeData::NAME => ffi::izs_make_data(p::<MakeData>(params)?.ea),
            MakeString::NAME => ffi::izs_make_string(p::<MakeString>(params)?.ea),
            Undefine::NAME => ffi::izs_undefine(p::<Undefine>(params)?.ea),
            MakeFunction::NAME => ffi::izs_make_function(p::<MakeFunction>(params)?.ea),
            DeleteFunction::NAME => ffi::izs_delete_function(p::<DeleteFunction>(params)?.ea),
            ToggleCollapse::NAME => ffi::izs_toggle_collapse(p::<ToggleCollapse>(params)?.ea),
            Undo::NAME => ffi::izs_undo(),
            Redo::NAME => ffi::izs_redo(),
            PyEval::NAME => {
                let m: PyEval = p(params)?;
                ffi::izs_py_eval(
                    cstr(&m.source)?.as_ptr(),
                    m.ea,
                    if m.force { 1 } else { 0 },
                    c"".as_ptr(),
                )
            }
            PyRunFile::NAME => {
                let m: PyRunFile = p(params)?;
                ffi::izs_py_eval(c"".as_ptr(), m.ea, 2, cstr(&m.path)?.as_ptr())
            }
            PyComplete::NAME => {
                let m: PyComplete = p(params)?;
                ffi::izs_py_complete(cstr(&m.word)?.as_ptr())
            }
            PluginList::NAME => ffi::izs_plugin_list(),
            PluginRun::NAME => {
                let m: PluginRun = p(params)?;
                ffi::izs_plugin_run(cstr(&m.plugin)?.as_ptr(), m.arg)
            }
            SigList::NAME => ffi::izs_sig_list(),
            SigApply::NAME => ffi::izs_sig_apply(
                cstr(&p::<SigApply>(params)?.names.join(
                    "
",
                ))?
                .as_ptr(),
            ),
            LuminaStatus::NAME => ffi::izs_lumina_status(),
            Lumina::NAME => {
                let m: Lumina = p(params)?;
                // The host's own gate: the shim refuses an unconfirmed push too.
                if m.action.is_push() && !m.confirmed {
                    return Err("Lumina push refused: it uploads your analysis to the server and needs explicit confirmation".into());
                }
                ffi::izs_lumina(m.action as i32, m.ea, m.confirmed)
            }
            Proximity::NAME => {
                let m: Proximity = p(params)?;
                ffi::izs_proximity(
                    m.ea,
                    m.up.min(8),
                    m.down.min(8),
                    m.fanout.clamp(1, 200),
                    m.max_nodes.clamp(1, 1000),
                    m.data,
                )
            }
            DbgStart::NAME => {
                let m: DbgStart = p(params)?;
                ffi::izs_dbg_start(
                    cstr(m.path.as_deref().unwrap_or(""))?.as_ptr(),
                    cstr(&m.args)?.as_ptr(),
                    cstr(&m.cwd)?.as_ptr(),
                    m.options.flags(),
                )
            }
            DbgProcesses::NAME => ffi::izs_dbg_processes(),
            DbgAttach::NAME => {
                let m: DbgAttach = p(params)?;
                ffi::izs_dbg_attach(m.pid, m.options.flags())
            }
            DbgDetach::NAME => ffi::izs_dbg_detach(),
            DbgTerminate::NAME => ffi::izs_dbg_terminate(),
            DbgState::NAME => ffi::izs_dbg_state(),
            DbgResume::NAME => {
                let m: DbgResume = p(params)?;
                ffi::izs_dbg_resume(m.how.code(), m.ea)
            }
            DbgPause::NAME => ffi::izs_dbg_pause(p::<DbgPause>(params)?.timeout_ms),
            DbgBpts::NAME => ffi::izs_dbg_bpts(),
            DbgBptAdd::NAME => {
                let m: DbgBptAdd = p(params)?;
                ffi::izs_dbg_bpt_add(
                    m.ea,
                    cstr(&m.kind)?.as_ptr(),
                    m.size,
                    cstr(&m.cond)?.as_ptr(),
                    cstr(&m.lang)?.as_ptr(),
                )
            }
            DbgBptDel::NAME => ffi::izs_dbg_bpt_del(p::<DbgBptDel>(params)?.ea),
            DbgBptEnable::NAME => {
                let m: DbgBptEnable = p(params)?;
                ffi::izs_dbg_bpt_enable(m.ea, m.enabled)
            }
            DbgBptCond::NAME => {
                let m: DbgBptCond = p(params)?;
                ffi::izs_dbg_bpt_cond(m.ea, cstr(&m.cond)?.as_ptr(), cstr(&m.lang)?.as_ptr())
            }
            DbgRegs::NAME => ffi::izs_dbg_regs(p::<DbgRegs>(params)?.all),
            DbgSetReg::NAME => {
                let m: DbgSetReg = p(params)?;
                ffi::izs_dbg_set_reg(cstr(&m.name)?.as_ptr(), m.value)
            }
            DbgMemRead::NAME => {
                let m: DbgMemRead = p(params)?;
                ffi::izs_dbg_mem_read(m.ea, m.len)
            }
            DbgMemWrite::NAME => {
                let m: DbgMemWrite = p(params)?;
                ffi::izs_dbg_mem_write(m.ea, cstr(&m.hex)?.as_ptr())
            }
            DbgThreads::NAME => ffi::izs_dbg_threads(),
            DbgSelectThread::NAME => ffi::izs_dbg_select_thread(p::<DbgSelectThread>(params)?.tid),
            DbgModules::NAME => ffi::izs_dbg_modules(),
            DbgStack::NAME => ffi::izs_dbg_stack(),
            DbgSymbolize::NAME => {
                let m: DbgSymbolize = p(params)?;
                let list: Vec<String> = m.eas.iter().map(|e| format!("{e:X}")).collect();
                ffi::izs_dbg_symbolize(cstr(&list.join(","))?.as_ptr())
            }
            DbgEval::NAME => ffi::izs_dbg_eval(cstr(&p::<DbgEval>(params)?.expr)?.as_ptr()),
            DbgExceptions::NAME => ffi::izs_dbg_exceptions(),
            DbgSetException::NAME => {
                let m: DbgSetException = p(params)?;
                ffi::izs_dbg_set_exception(m.code, m.stop, m.pass)
            }
            other => return Err(format!("unknown method '{other}'")),
        };
        result(rc)
    }
}

/// Exits the host when the UI process dies. Analysis can run for minutes
/// without reading stdin, so pipe EOF alone would leave an orphan.
fn watch_parent() {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
        fn GetCurrentProcess() -> *mut c_void;
        fn TerminateProcess(handle: *mut c_void, code: u32) -> i32;
    }
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let Some(pid) = std::env::var("IDA_ZUI_PARENT")
        .ok()
        .and_then(|p| p.parse().ok())
    else {
        return;
    };
    std::thread::spawn(move || unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        WaitForSingleObject(h, u32::MAX);
        TerminateProcess(GetCurrentProcess(), 0);
    });
}

fn main() {
    watch_parent();
    let (mut input, output) = unsafe {
        let mut hin = std::ptr::null_mut();
        let mut hout = std::ptr::null_mut();
        let rc = ffi::izs_take_stdio(&mut hin, &mut hout);
        if rc != 0 {
            eprintln!("ida-host: cannot reserve stdio for the protocol ({rc})");
            std::process::exit(2);
        }
        // SAFETY: freshly duplicated handles owned by nobody else.
        (
            BufReader::new(File::from_raw_handle(hin)),
            File::from_raw_handle(hout),
        )
    };

    // `--loader-binary`: force IDA's raw "Binary file" loader for every open.
    let binary_loader = std::env::args().any(|a| a == "--loader-binary");
    let init = unsafe { result(ffi::izs_init(binary_loader)) };
    if let Err(e) = init {
        eprintln!("ida-host: {e}");
        std::process::exit(3);
    }

    *OUT.lock().unwrap() = Some(output);
    unsafe { ffi::izs_set_emitter(emit_event) };

    // Requests are read on a thread of their own so that `interrupt` can be
    // answered while the main thread is busy inside a script.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        loop {
            line.clear();
            match input.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line).trim_end().to_owned();
            if let Ok(req) = serde_json::from_str::<Request>(&text)
                && (req.method == Interrupt::NAME || req.method == AnalysisStop::NAME)
            {
                let rc = unsafe {
                    if req.method == AnalysisStop::NAME {
                        ffi::izs_analysis_stop()
                    } else {
                        ffi::izs_interrupt()
                    }
                };
                let resp = if rc == 0 {
                    Response {
                        id: req.id,
                        ok: None,
                        err: None,
                    }
                } else {
                    Response {
                        id: req.id,
                        ok: None,
                        err: Some(format!("nothing to interrupt ({rc})")),
                    }
                };
                write_line(&serde_json::to_vec(&resp).unwrap_or_default());
                continue;
            }
            if tx.send(text).is_err() {
                break;
            }
        }
    });

    // While a debuggee runs the loop wakes up between requests to poll the
    // debugger for events; otherwise it blocks on stdin as before.
    loop {
        let text = if unsafe { ffi::izs_dbg_running() } != 0 {
            match rx.recv_timeout(std::time::Duration::from_millis(5)) {
                Ok(t) => t,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    unsafe { ffi::izs_dbg_poll() };
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(t) => t,
                Err(_) => break,
            }
        };
        let req: Request = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ida-host: malformed request: {e}");
                continue;
            }
        };
        let closing = req.method == Close::NAME;
        let origin = if req.method == PyEval::NAME
            || req.method == PyRunFile::NAME
            || req.method == PluginRun::NAME
        {
            2
        } else {
            1
        };
        unsafe { ffi::izs_begin_request(origin) };
        let outcome = if closing {
            let save = serde_json::from_str::<Close>(req.params.get())
                .map(|c| c.save)
                .unwrap_or(false);
            unsafe { ffi::izs_close(save) };
            Ok("null".to_owned())
        } else {
            dispatch(req.method, req.params.get())
        };
        // Pending events go out before the reply, so a client that has the
        // reply has also seen everything the request caused.
        unsafe { ffi::izs_end_request() };
        let outcome = outcome.and_then(|json| {
            serde_json::value::RawValue::from_string(json)
                .map_err(|e| format!("shim produced invalid JSON: {e}"))
        });
        let resp = match outcome {
            Ok(raw) => Response {
                id: req.id,
                ok: Some(raw),
                err: None,
            },
            Err(err) => Response {
                id: req.id,
                ok: None,
                err: Some(err),
            },
        };
        if !write_line(&serde_json::to_vec(&resp).unwrap_or_default()) || closing {
            break;
        }
    }
    unsafe { ffi::izs_close(false) };
}
