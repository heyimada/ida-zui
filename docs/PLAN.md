# Roadmap

Phases run in order, because most of them touch `workspace.rs` and `shim.cpp`. The Phase 0 spike ran alongside Phase 1.

| Phase | Scope | Status |
|---|---|---|
| 0 | **Feasibility spike** (scratch C++ against idalib): debugger (start, breakpoints, step, registers, memory), IDAPython eval, Lumina pull/push, FLIRT list/apply, IDB/IDP event hooks, auto-analysis progress. Report what works headless. | done: see `IDALIB_FINDINGS.md` |
| 1 | **Session safety**: close the DB on any window close, treat fresh analysis as dirty, ask before open/drop replaces a dirty DB, verify the save prompt, scale test on a large DLL and a 32-bit binary, release build. | done (open-over-dirty shares the tested code path but was not driven in the UI) |
| 2 | **Shell**: in-app menu bar (File, Edit, Jump, Search, View, Debugger, Options, Windows, Help), loader-options open dialog, settings file with themes and keymaps, recent files. Also replace SendKeys with an in-app automation channel. | done (the Debugger menu waits for phase 7) |
| 3 | **Editors**: multi-line code editor component (`crates/zui-editor`, in progress), local types / struct / union / enum editors, stack frame view and editor. | done (limits under Partial in README) |
| 4 | **Multiple databases per window**: document tabs, one host per tab. Plus review polish: listing indent, prototype name, graph overlays. | done |
| 5 | **IDAPython console** with output; IDB hooks drive refresh for external changes; analysis progress. | done: host events (`log`, `db_changed`, `analysis`), Python console, script file, interrupt, stepped analysis with progress, Settings Analysis and Python. Gaps in README |
| 6 | **Signatures and Lumina**: FLIRT list and apply, Lumina pull and push, proximity (call graph) view, graph edge routing around blocks. | done: FLIRT list and apply, Lumina pull and guarded push, proximity view, edge routing |
| 6b | **Plugins and scrollbars**: Edit › Plugins menu, Plugins window, running plugins and their hotkeys (`GAPS.md` G1); overlay scrollbars in every scrollable view (M3); graph trunks (L5) and proximity readability (L6). | done |
| 7 | **Debugger**: local Windows debugging. Start, attach, breakpoints, step, run to cursor, registers, stack, memory, threads, modules, debugger-aware listing. | done: host (`shim/debug.cpp`, `dbg_*` requests, `debug` events, rebase restore), UI (Debugger menu, toolbar, panels, markers, Settings > Debugger), `debug_e2e` against `scripts/debuggee`. Gaps in README |
| 2b | **Restyle** to the chosen Workbench × Native Pro design (six reference screens: main, graph, menus, settings, welcome, palette). | done |
| 8a | **Polish, first pass** (`GAPS.md`): stopping an analysis on close (S1) and the host-exit check (S3), listing columns kept out of the `.i64` (S4), drop automation (S2), paged-listing key queue (L1), graph zoom to 3% (L7), proximity clicks (L8), per-declaration type errors (E1), struct-offset xrefs E2E (E3), Local Types overflow menu (E5), Python Tab completion and streamed output (P2, P3), Settings About (M4), bottom-panel tab overflow (D1), rebase-following editor tabs (D2), debugger UI paths driven (D3). L4 did not reproduce. | done |
| 8b | **Final sweep**: remaining `GAPS.md` items (D4 to D7, L2, L3, E2, P1, P4, M5 to M7, H2), first-run setup screen (Locate IDA, missing host, startup errors), `scripts/package.ps1` release zip, IDA key alignment (Ctrl+X / Ctrl+J, Ctrl+T / Ctrl+B, Shift+F3..), walkthrough as a new IDA user, README audit and "Coming from IDA". | done: E4, M2 and H1 remain (see GAPS.md) |
| 8 | **Polish** (what 8a left, then the rest): everything in `GAPS.md` (P1 first), plus overlay scrollbars from the spec; Local Types toolbar overflow at narrow widths; full-width graph check; Settings sections Analysis/Debugger/Python/About filled in; text-search performance, README rewrite, final E2E and UI sweep. | superseded by 8a and 8b |

## Phase 5 analysis timing (debug host, open + analysis, fresh copies)

`IZS_AUTO_MODE=native` is the old path (IDA analyses inside `open_database`), `wait` is `auto_wait()` after opening, `step` (default) is the stepped loop with progress events. Interleaved rounds on a busy machine:

| Sample | native | wait | step |
|---|---|---|---|
| find.exe (43 functions) | ~1.0 s | ~1.0-1.1 s | ~1.0-1.1 s |
| shell32.dll (34192 functions) | 123 s, 117 s | 127 s, 114 s | 111 s, 106 s |

Stepping is not slower (an earlier version that called `auto_is_ok()` before each of ~10 million steps was). Quiet-machine runs before the change: ~77-82 s. The e2e dump of `find.exe` is identical between modes except the analysis-event checks.
