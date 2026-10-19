# Known gaps for later polish

These gaps came from development reviews. Each entry gives the development phase it came from (see `PLAN.md`), a priority, and a suggested fix.

- **P1:** visible to users, or a data-safety issue.
- **P2:** rough edge.
- **P3:** nice to have.

The last section lists dropped and already-covered items so none of them gets lost.

## Session lifecycle and safety

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| S1 | Closing a database tab or the window while analysis is running waits up to 15 s for the host, then kills it. The analysis is lost. | 1, 4 | P1 | During analysis, offer "Stop analysis and close" or "Cancel". Request a cooperative stop between `auto_make_step` calls, now that analysis is stepped. Kill only as a last resort. |
| S2 | Opening a file over a dirty database through the file picker or drag & drop was never driven in the UI. It shares the close-prompt code path. | 1 | P2 | Add an automation command that simulates a drop or a picker result, then E2E it. |
| S3 | Nobody saw the host processes exit after closing *all* tabs. The last run's check ran after the app had already exited. | 4 | P1 | Re-run the `hostsgone` step in the final sweep. Assert in `scripts/ui-capture.ps1`. |
| S4 | Saving writes the listing indent and comment column settings into the user's `.i64`. ida-zui's display preferences leak into the database. | 4 | P2 | Restore the original `inf` values before `save_database`, or apply them only for rendering. |

## Listing, graph and hex

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| L1 | Rapid PageUp that outruns the async page-in clamps at the top of the buffer, so some key presses do nothing. | 1 | P2 | Queue the pending scroll distance and apply it when the chunk arrives. Prefetch further ahead while keys repeat. |
| L2 | Graph blocks that start with `var_xx = …` frame lines still look indented. Those lines sit at column 0 while the instructions are indented. | 4 | P3 | Show frame-variable lines in a muted header section of the entry block, or trim them separately. |
| L3 | The graph doesn't fold long blocks. The design shows an "N earlier instructions" fold chip. | 2b | P3 | Collapse blocks over about 40 lines around the caret line, and expand on click. |
| L7 | Graph zoom-to-fit stops at 15%, so large functions (FIND::Initialize) never fit fully on screen. | 6 | P2 | Lower the minimum zoom to about 3% and draw blocks as filled boxes without text below about 30%. The minimap covers navigation. |
| L8 | In the proximity view, the +/− expand buttons and double-click-to-navigate were never driven by clicks. The Data toggle was only checked while off. | 6 | P2 | Add automation `click` steps on the node buttons and E2E the expand/collapse plus the Data toggle on. |
| L4 | One run showed the view switched to Pseudocode after Ctrl+1 and Ctrl+S. It hasn't reproduced since. | 4 | P2 | Watch for it in the final sweep. If it reappears, check whether a key-context leak sends Ctrl+S to the segmented control. |

## Editors and types

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| E1 | IDA's type parser gives no line numbers. Errors are located by searching for the named token, and errors without a token land on the previous error's position. | 3 | P2 | Parse the declaration incrementally, one declaration at a time, and attribute each error to its declaration's line range. |
| E2 | An unterminated `/*` is silently accepted by IDA's parser. No warning is shown. | 3 | P3 | Lint the text client-side with the editor's highlighter state before applying. |
| E3 | Struct-member xrefs are implemented, but only an empty result was ever exercised. The sample has no struct-offset operands. | 3 | P2 | Add an E2E case that applies a struct offset to an operand, then queries member xrefs. |
| E4 | IME composition in the code editor and text inputs is untested, for example Japanese or German dead keys. | 3 | P2 | Test it manually once on a real keyboard. The automation channel bypasses IME. |
| E5 | The Local Types toolbar clips the "Delete" button at narrow widths. | 2b | P2 | Collapse secondary buttons into an overflow menu below a width threshold. |

## Python console

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| P1 | There's no IDC console, only Python. | 5 | P3 | Add a language switch (Python / IDC) using the IDC extlang `eval_snippet`. |
| P2 | The console has no completion. | 5 | P2 | Complete on Tab through Python's `rlcompleter` evaluated in the host. Show results in a popover. |
| P3 | Console output appears only when a command returns. The Output tab streams, but the console doesn't. | 5 | P2 | Forward the `log` events of a running eval into the console as they arrive. |
| P4 | Cancel interrupts only while Python executes bytecode. A long C call or `time.sleep` blocks until it returns. | 5 | P3 | Document the limitation in the UI. Offer "Kill host" after a timeout, with the unsaved-changes warning. |

## Shell, menus and settings

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| M2 | A real mouse click on the database chips in the title strip (the custom drag region) is untested. | 4 | P1 | Test by hand. Make sure the chips aren't part of the OS caption hit-test area. |
| M4 | The Settings sections Debugger and About are disabled placeholders. Analysis and Python were filled in by Phase 5. | 2b | P2 | Phase 7 fills Debugger. About can reuse the existing About dialog content. |
| M5 | Recent files show no processor or architecture badge, though the design shows one. | 2b | P3 | Store the processor and bitness in the recent-files entry when a database opens. |
| M6 | The palette's "Names" group is mostly empty, because names at function addresses are deduplicated into Functions. | 2b | P3 | Show non-function names only (data, labels), and say so in the group header. |
| M7 | The High contrast and IDA Classic themes were never reviewed against a mockup. | 2b | P3 | Review both by screenshot in the final sweep and tune their token values. |

## Loader and host

| # | Gap | Source | Pri | Suggested fix |
|---|---|---|---|---|
| H1 | `tricore`, `msp430` and `xtensa` don't round-trip headless through the `DEFAULT_PROCESSOR` workaround, so the open dialog doesn't offer them. | 2 | P3 | Investigate the processor module names and IDAUSR mapping per module. Add each one back once verified. |
| H2 | The shell32 E2E dump is 5.7 MB, because the full lists and pseudocode are written out. | 1 | P3 | Cap the list sections in the dump, for example the first 200 entries plus counts, for samples above a size threshold. |

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
