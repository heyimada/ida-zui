# idalib: what works headless

All results below were verified with standalone C++ programs against an IDA Pro 9.x installation and its SDK, before the matching feature was built into the host.

## General

- The SDK poisons `printf`, `fflush` and `getenv`. Use `qprintf`, `qflush` and `qgetenv` instead.
- With `enable_console_messages(true)`, all `msg()` output goes to stdout. That includes Python `print` and plugin tracebacks. The host redirects stdout to stderr.
- Plugins from the user's IDAUSR load into every host. IDAUSR accepts several paths separated by `;`, so we can prepend our own directory.
- IDA tries Lumina after analysis and logs `lumina: connect ... refused`. This is harmless.
- **Stale unpacked files:** a crash leaves `.id0/.id1/.nam/.til` next to the input, and the next `open_database` silently reuses them.

## Debugger: works

```cpp
load_debugger("win32", false);
add_bpt(ea);
start_process(path, args, startdir);                  // returns 1, state DSTATE_RUN
auto c = wait_for_next_event(WFNE_ANY | WFNE_USEC | WFNE_SILENT, 50000);  // 50 ms poll; DEC_TIMEOUT=0, DEC_NOTASK=-2
const debug_event_t *ev = get_debug_event();          // ev->eid(), ev->ea, ev->tid
get_reg_val("RIP", &u64); read_dbg_memory(ea, buf, n);
get_thread_qty(); getn_thread(i); getn_thread_name(i);
get_first_module(&m); get_next_module(&m);
step_into(); step_over(); continue_process(); suspend_process(); exit_process();
```

- Events seen: `PROCESS_STARTED`, `LIB_LOADED`, `THREAD_STARTED`, `BREAKPOINT`, `STEP`, `THREAD_EXITED`, `PROCESS_EXITED`, `PROCESS_SUSPENDED`. Breakpoint and step events leave the process in `DSTATE_SUSP` (-1).
- Polling with a short timeout doesn't block. Ordinary IDB queries work between polls, even while the debuggee runs.
- **ASLR rebase:** at process start the IDB is rebased to the runtime image base and stays rebased after exit. Never save it like that. Rebase back, or warn the user.
- `read_dbg_memory` shows `CC` at software breakpoints. `get_byte` shows the original byte.
- Continuing from a breakpoint delivers a `STEP` event first, then the process runs.
- `suspend_process` sometimes never completes (seen on a 2-thread target). Poll with a timeout and offer terminate.
- Attach and detach were not tested.

## IDAPython: works

```cpp
extlang_object_t py = find_extlang_by_name("Python");
py->eval_snippet("import idaapi\nprint(idaapi.get_func_qty())\n", &err);   // err = traceback
py->eval_expr(&rv, BADADDR, "idaapi.get_func_qty()", &err);               // rv.i64 or VT_STR
```

- Capture output with an `HT_UI` listener on `ui_msg`: `fmt = va_arg(va, const char*)`, then `va_list` and `qvsnprintf`. This catches `print`, stderr and loader messages.
- `idaapi.get_inf_structure` is gone in IDA 9.x. Use `ida_ida.inf_*` instead.

## Lumina: partial

- The SDK has no API for it. The kernel registers 18 UI actions, for example `LuminaPullAllMds`, `LuminaPushAllMds`, `LuminaPullMd` and `LuminaPushMd`. They run through `process_ui_action(name)`.
- Lumina uses whatever server IDA's own settings name. Pull and push results are read from IDA's log, and an unreachable server only shows up there.
- The UI exposes the actions and reports IDA's result. It never changes the user's Lumina configuration.

## FLIRT: works

- Query signatures with `get_idasgn_qty()`, `get_idasgn_desc(&name, &opt, i)` (which returns the match count) and `calc_idasgn_state(i)`.
- Apply one with `plan_to_apply_idasgn("vc64rtf")`, then `auto_wait()`.
- Signature files are in `<IDADIR>\sig\<proc>\*.sig`. `pc` has 156 of them.

## Event hooks: work

`hook_event_listener(HT_IDB, &listener, nullptr)` fires `renamed`, `cmt_changed`, `ti_changed` and `byte_patched`. This holds whether the change comes from C++ or from Python.

## Auto-analysis progress: works

```cpp
open_database(path, false); enable_auto(true);
while (!auto_is_ok()) { if (!auto_make_step(0, BADADDR)) break; get_auto_display(&d); /* d.type = AU_*, d.ea */ }
auto_wait();
```

- Queue order: `AU_CODE` 20, `AU_PROC` 30, `AU_FCHUNK` 38, `AU_USED` 40, `AU_TYPE` 50, `AU_LIBF` 60, `AU_FINAL` 200.
- Derive progress from the queue stage plus `d.ea` within `[min_ea, max_ea]`.
- IDB queries can be served between steps.

## Loader options: open_database args do not work

- Any non-empty `args` to `open_database` exits the process with code `0xDEAD`.
- `init_library(argc, argv)` with `argv[0]="ida"` and **one** switch: `-TBinary file` (a single token) forces the loader. `-L<log>` is also accepted. `-p`, `-b`, `-A` and others fail.
- **Choosing the processor:** add a `DEFAULT_PROCESSOR = { "ext" : "proc" }` entry in `<IDAUSR>\cfg\ida.cfg` keyed by file extension, then open a copy named `x.<ext>` with `-TBinary file`. This was verified for ARM.
- `set_processor_type` after load fails.
- **Base address:** `rebase_program(delta, MSF_FIXONCE)` after load works.

## Multiple databases

A second `open_database` in the same process returns -1. Keep one host process per database.
