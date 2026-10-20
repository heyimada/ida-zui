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

## Editors and types

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| E4 | IME composition in the code editor and text inputs is untested, for example Japanese or German dead keys. | 3 | P2 | Test it manually once on a real keyboard. The automation channel bypasses IME. Code review: both `TextInput` and the zui-editor `Editor` implement the full `EntityInputHandler` composition protocol (marked range, `replace_and_mark_text_in_range`, `unmark_text`, `bounds_for_range` for the candidate window), following gpui's reference input; what is unverified is only the platform path. |

## Python console

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|

## Shell, menus and settings

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|

## Debugger

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| D8 | First or last chance of an exception is inferred, and hit counts are the UI session's own count, not IDA's. | 7 | P3 | Left as is: IDA's API gives no chance flag and no hit count to read (noted in the README). |

## Loader and host

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| H1 | Resolved: `tricore`, `msp430` and `xtensa` came out as metapc because the processor map used commas between entries (only the first is read) and keys longer than nine characters are ignored. Fixed in `raw_binary` / `processor_extension`; all three are in the open dialog and in the protocol E2E. | 2 | - | Done. |

## Resolved in Phase 8b

- **D4 Process memory:** `Debugger > Hex view: process memory` switches the hex view to `dbg_mem_read` pages (re-read at every stop, "PROCESS MEMORY" badge). Verified on the self-built debuggee: `ida_dbg.write_dbg_memory` wrote `IZSTEST!` at RIP from the Python console and the hex view showed it with the toggle on.
- **D5 Vector registers:** the Registers panel lists xmm/ymm/zmm (>= 128 bit) below the general registers, read-only (screenshot).
- **D6 Thread IPs and names:** `dbg_threads` selects each thread, reads its IP and restores the selection (E2E: every thread has an IP, the current one equals RIP, the selection is unchanged). Threads whose "name" is a raw address show as `thread <tid>`.
- **D7 Pseudocode marker:** by design it uses the exact ctree item address when Hex-Rays has one at the instruction pointer and the enclosing statement otherwise; nothing finer exists in the ctree. Documented.
- **L2 Frame lines:** `proc` and `var_x = ... ptr` lines no longer count as instructions when the block's shared indent is trimmed (unit test), so entry blocks are flush (screenshot).
- **L3 Long blocks:** blocks over 40 lines fold to 14 head + chip + 14 tail lines; the chip, Enter on it, or a jump into the hidden part expands the block (screenshot).
- **E2 Unterminated comment:** `declerr::unterminated_comment` (tests first) marks the line and warns on Apply (screenshot).
- **P1 IDC console:** a Python / IDC button in the console header; IDC runs through `ida_expr.eval_idc_expr` / `compile_idc_snippet` (screenshots: `2+2`, `auto x = 5; ... Message(...)`, errors).
- **P4 Kill host:** 4 s after Cancel the strip offers "Kill host..." (second click kills, with the unsaved warning); the unpacked database files are removed when a packed `.i64` exists, so an unpacked-only database is never deleted (screenshots).
- **M5 Recent badge**, **M6 Names group** (non-function names, header says so; an exact name match now ranks first), **M7 themes** (High contrast and IDA Classic tuned; a unit test asserts 4.5:1 for every text token on every surface).
- **H2:** list sections of the E2E dump are capped at 200 entries plus the count (not re-run on shell32.dll).
- **Proximity, off-screen nodes (L8 leftover):** after a + expansion or opening a "+N more" chip the view pans, and zooms out down to 30% if needed, until the new nodes and their anchor are in view (it does nothing when they already are). Screenshots: expanding `__mainCRTStartup`'s callees, and opening the callee chip of `FIND::Initialize`.
- **Proximity, Data nodes:** data nodes did render (yellow strip, listed in the legend) but never made the per-node fan-out cut, because functions filled it. The host now gives data targets a quota of their own (a third of the fan-out) when Data is on. Screenshots with Data on: `asc_140004504`, `aI`, `aV`, `aC` and `__xi_z`, `__native_startup_state` appear with the data colour, after opening the chip or expanding.
- **M2 title chips:** clicking the sort.exe and find.exe chips switches databases (in-app mouse events, screenshots m2_sort/m2_find). The chips are siblings of the `Drag` control areas, never inside one, so the OS hit-test treats them as client area.
- **Comment key on any layout:** `:` is bound by character, so it fires whichever physical key types it (checked with a simulated German-layout Shift+.; screenshot colon_de).
- Left for a human: **E4** (IME composition).

## Resolved in Phase 8a

- **S1 Stop analysis on close:** closing a tab or the window while an analysis runs asks "Stop analysis and close" / "Cancel" (a tab with unsaved edits keeps Save / Don't save / Cancel and stops the analysis first). The `analysis_stop` request is answered by the host's reader thread; the stepped loop checks a sticky flag between `auto_make_step` calls and skips `auto_wait`. The 15 s kill stays as the last resort. `stop_e2e` (in `e2e.ps1`): a `shell32.dll` copy is stopped mid-analysis, the stop is answered within 2 s, `open` returns, `close` is acknowledged, the host exits and no `.id0/.id1/.id2/.nam/.til/.i64` is left. UI: dialog screenshots for tab close and window close, then no host and no files. The open reply that arrives after a close is dropped.
- **S3 Hosts exit after the last tab closes:** `hostsgone[:ms]` in `ui-capture.ps1` now waits for the remembered hosts while the app is still alive and fails the script (exit 1) if one is left or the app already exited. Two tabs closed through Ctrl+W (now Ctrl+Shift+W): both hosts gone, app alive, no leak found.
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
