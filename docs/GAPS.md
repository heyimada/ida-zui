# Known gaps for later polish

These gaps came from development reviews. Each entry gives the development phase it came from (see `PLAN.md`), a priority, and a suggested fix.

- **P1:** visible to users, or a data-safety issue.
- **P2:** rough edge.
- **P3:** nice to have.

The last section lists dropped and already-covered items so none of them gets lost.

## Session lifecycle and safety

Nothing open: S1 to S4 are resolved (see Phase 8a below).


## Listing, graph and hex

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| L2 | Graph blocks that start with `var_xx = …` frame lines still look indented. Those lines sit at column 0 while the instructions are indented. | 4 | P3 | Show frame-variable lines in a muted header section of the entry block, or trim them separately. |
| L3 | The graph doesn't fold long blocks. The design shows an "N earlier instructions" fold chip. | 2b | P3 | Collapse blocks over about 40 lines around the caret line, and expand on click. |

## Editors and types

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| E2 | An unterminated `/*` is silently accepted by IDA's parser. No warning is shown. | 3 | P3 | Lint the text client-side with the editor's highlighter state before applying. |
| E4 | IME composition in the code editor and text inputs is untested, for example Japanese or German dead keys. | 3 | P2 | Test it manually once on a real keyboard. The automation channel bypasses IME. |

## Python console

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| P1 | There's no IDC console, only Python. | 5 | P3 | Add a language switch (Python / IDC) using the IDC extlang `eval_snippet`. |
| P4 | Cancel interrupts only while Python executes bytecode. A long C call or `time.sleep` blocks until it returns. | 5 | P3 | Document the limitation in the UI. Offer "Kill host" after a timeout, with the unsaved-changes warning. |

## Shell, menus and settings

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| M2 | A real mouse click on the database chips in the title strip (the custom drag region) is untested. | 4 | P1 | Test by hand. Make sure the chips aren't part of the OS caption hit-test area. |
| M5 | Recent files show no processor or architecture badge, though the design shows one. | 2b | P3 | Store the processor and bitness in the recent-files entry when a database opens. |
| M6 | The palette's "Names" group is mostly empty, because names at function addresses are deduplicated into Functions. | 2b | P3 | Show non-function names only (data, labels), and say so in the group header. |
| M7 | The High contrast and IDA Classic themes were never reviewed against a mockup. | 2b | P3 | Review both by screenshot in the final sweep and tune their token values. |

## Debugger

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| D4 | The listing and hex view show the database's bytes, not process memory, while debugging; self-modifying or unpacked code looks stale. | 7 | P2 | An optional "show process memory" mode, or a refresh-from-memory command for a range (IDA: refresh memory, take memory snapshot). |
| D5 | xmm/ymm registers are fetched but not shown. | 7 | P3 | A vector register section in the Registers panel. |
| D6 | The Threads list has no per-thread instruction pointer. | 7 | P3 | Read each thread's RIP/EIP while suspended and add a column. |
| D7 | The pseudocode instruction-pointer marker snaps to the nearest statement address, not an exact instruction. | 7 | P3 | Use the ctree item addresses to mark the exact instruction's statement. |
| D8 | First or last chance of an exception is inferred, and hit counts are the UI session's own count, not IDA's. | 7 | P3 | Read IDA's breakpoint hit count; keep the inference for exceptions (the API gives none). |

## Loader and host

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| H1 | `tricore`, `msp430` and `xtensa` don't round-trip headless through the `DEFAULT_PROCESSOR` workaround, so the open dialog doesn't offer them. | 2 | P3 | Investigate the processor module names and IDAUSR mapping per module. Add each one back once verified. |
| H2 | The shell32 E2E dump is 5.7 MB, because the full lists and pseudocode are written out. | 1 | P3 | Cap the list sections in the dump, for example the first 200 entries plus counts, for samples above a size threshold. |

## Resolved in Phase 8a

- **S1 Stop analysis on close:** closing a tab or the window while an analysis runs asks "Stop analysis and close" / "Cancel" (a tab with unsaved edits keeps Save / Don't save / Cancel and stops the analysis first). The `analysis_stop` request is answered by the host's reader thread; the stepped loop checks a sticky flag between `auto_make_step` calls and skips `auto_wait`. The 15 s kill stays as the last resort. `stop_e2e` (in `e2e.ps1`): a `shell32.dll` copy is stopped mid-analysis, the stop is answered within 2 s, `open` returns, `close` is acknowledged, the host exits and no `.id0/.id1/.id2/.nam/.til/.i64` is left. UI: dialog screenshots for tab close and window close, then no host and no files. The open reply that arrives after a close is dropped.
- **S3 Hosts exit after the last tab closes:** `hostsgone[:ms]` in `ui-capture.ps1` now waits for the remembered hosts while the app is still alive and fails the script (exit 1) if one is left or the app already exited. Two tabs closed through Ctrl+W: both hosts gone, app alive, no leak found.
- **S4 Listing columns in the `.i64`:** the original indent, comment column and address prefix are remembered after open, put back before `save_database` and before a saving `close`, and re-applied after `save`. Protocol E2E: after a save with the UI's columns set, a fresh host reads IDA's own 16/40 from the database.
- **S2 Open over a dirty database:** that case no longer exists. Since Phase 4 every open gets a tab of its own (a drop of an already-open file switches to it), so nothing is replaced and there is no prompt. The automation command `drop <path>` feeds gpui's file-drop events into the same handler as a real drop; E2E by screenshots: a drop over a dirty database adds a tab and leaves the first dirty, a repeated drop reports "already open", and closing the window asks once for both.
- **L1 PageUp/PageDown past the buffer:** a move that outruns the paged buffer is kept as owed rows and applied when the next chunk arrives (`step_cursor`, unit-tested); the prefetch margin grew from 60 to 100 rows. The automation channel cannot outpace the host round trip, so the end-to-end check (30 pages down, 60 up, reaching the start of the segment) shows no regression rather than the race itself.
- **L7 Graph zoom:** minimum zoom 3%, boxes without text below 30% (filled with the raised surface colour). Screenshot: "Fit graph" on `FIND::Initialize` shows all 47 blocks.
- **L8 Proximity:** driven by automation clicks: the + button on a caller and on a callee node (new nodes appear), the Data toggle (turns blue, callee counts grow from 27 to 40), the "+N more" chip and double-click on a node (jumps to it in the listing). Left open: new nodes after an expansion can lie outside the view.
- **E1 Per-declaration type errors:** `declerr::split_declarations` / `locate_in` (unit tests written first). When a multi-declaration text fails, each declaration is applied alone (the good ones are idempotent) and its messages stay inside its own lines. Protocol E2E: a text with a good, a syntax-error and an undefined-type declaration gives error lines inside the right declarations.
- **E3 Struct-member xrefs:** protocol E2E applies a struct offset to a real `[reg+18h]` operand (`op_stroff`), lets the analysis queue run, and `udt_xrefs` of that member lists the operand (and another member does not). Note: `op_stroff` alone links only the struct; IDA adds the member xref when the analysis processes the operand.
- **E5 Local Types toolbar:** below 780 px the secondary buttons fold into a menu (New type, Import, Open editor, Delete); screenshots of the narrow toolbar and the open menu.
- **P2 Console completion:** `py_complete` (Python's `rlcompleter` in the host) and Tab in the console: inserts the common prefix, lists the candidates above the input and Tab again cycles them. Protocol E2E and screenshot.
- **P3 Streamed console output:** the host's `log` events are shown while a script runs and replaced by the typed output when it returns (screenshot at 7 s of an 8 s script).
- **M4 Settings About:** the About section shows the About dialog's content (IDA version and paths from the host); the Debugger section was filled in Phase 7.
- **L4:** the Ctrl+1, Ctrl+S flip to Pseudocode did not reproduce in four rounds (the view stayed Linear). Left as noted.

- **D1 Bottom panel tab strip:** tabs that do not fit fold into a "More tabs" chevron menu; the active tab always stays on the strip (`fit_tabs`, unit-tested). Screenshot: Events and Exceptions in the menu at the default width.
- **D2 Stale editor tabs on rebase:** `Rebased { delta }` now shifts the address of open Frame and Proximity tabs and reloads them (Struct and Enum tabs are keyed by name). Screenshot: a frame tab opened before the process starts still shows the frame afterwards.
- **D3 Debugger UI paths driven by automation** (self-built `izs_debuggee` only): the attach picker (filter, Enter: attached, suspended in ntdll), inline register edit (RBX set to 0x4D2), flag toggle (CF), breakpoint condition editor (`RCX == 3` shown in the list), exception Stop and Pass toggles (the Stop box cleared and Pass set). The earlier screenshots call stack, threads, modules, events, exceptions and ended were opened and look right. Not bug-free elsewhere: the Threads names for system threads show raw addresses (IDA gives no name).

## Resolved in Phase 6b

- **G1 Plugins:** `plugin_list` / `plugin_run`, Edit > Plugins with hotkeys in the keymap, Plugins window, palette entries; Qt-only plugins fail with a clear message.
- **M3 Scrollbars:** overlay scrollbar component on every scrollable surface, including both axes on the graph canvases.
- **L5 Edge bundling:** loops and long edges into one block share a trunk and one arrival port (unit-tested).
- **L6 Proximity readability:** code font at scale, fit on open, rows and "+N more" chips.

## Already covered elsewhere, or dropped

- **Debugger (Phase 7) leftovers:** the listing and hex view show database bytes rather than process memory; xmm/ymm registers and per-thread instruction pointers are not shown; the exception chance is inferred; pseudocode markers snap to the nearest statement address; open frame/struct editor tabs keep stale addresses across a rebase; tab strip of the bottom panel clips on narrow windows.
- **Debugger pitfalls** (`suspend_process` sometimes not completing, the ASLR rebase leaving the IDB rebased): Phase 7 must handle both. See `IDALIB_FINDINGS.md`.
- **Lumina push untestable:** by design, tests must never push. Phase 6 implements it behind a confirmation.
- **Graph only checked in the split pane, and edges crossing blocks:** Phase 6 covers edge routing plus a full-width check.
- **Local Types header not matching the editor:** fixed in Phase 2b.
- **Real-keyboard menu access (M1):** verified by hand that Alt+F opens File, F10 toggles focus to the menu bar and back, and a bare Alt tap focuses the bar.
- **Prototype showing `FIND__SearchFiles`, listing indent, graph overlays:** fixed in Phase 4.
- **Dropped as not worth fixing:**
  - The focus ring has no 1 px offset (cosmetic spec deviation).
  - Mnemonic letters in dropdowns are chosen automatically (they work as keys).
  - Some Phase 2b unit tests were written alongside the code. This is a process note, not a product gap.
  - The open-with-options dialog opens metapc directly without a copy. That is intended behavior.
