# ida-zui

**A modern, GPU-rendered front end for IDA Pro, written in Rust.** The IDA kernel runs headless through idalib in its own process; the UI is built on [zui](https://github.com/zeronsh/zui), a fork of Zed's gpui. No Qt anywhere.

![ida-zui main window, dark theme: linear listing beside Hex-Rays pseudocode, function list, inspector and xrefs](docs/images/main-dark.png)

![ida-zui main window, light theme](docs/images/main-light.png)

> **Status: early, but every feature listed here works.** Disassembly, pseudocode, graph, hex, type editors, a Python console, signatures, plugins and multi-database windows work today. Local Windows debugging works too. See [Status and known gaps](#status-and-known-gaps) and [docs/GAPS.md](docs/GAPS.md).

## Features

- **Linear listing, graph, hex and Hex-Rays pseudocode**, as document tabs: open as many as you like (`View > New tab`, `Ctrl+click` on a name), put two panes side by side (`View > Move tab to other pane`, the toolbar pseudocode-pane toggle, or drag a tab across), reorder tabs by dragging, cycle with `Ctrl+PgDn` / `Ctrl+PgUp`. A tab can be pinned to its place (it keeps its own back / forward history) or follow the cursor. Open tabs and panes come back when the database is reopened. `View > Move tab to a new window` tears a view off into a window of its own (second monitor); closing it docks the tab again; windows left open come back at their place after a restart. The divider between two panes drags (double-click resets it), tabs have a right-click menu, and the sidebar width is draggable and remembered. Disassembly comes from IDA's own line generator, so colours, comments and xref annotations match IDA. Graph blocks over 40 lines fold their middle into a "N lines hidden" chip that expands on click (or when the caret needs it).
- **Graph view** with a layered layout, routed edges (taken, not taken, back edges), zoom, pan and a minimap.

  ![Graph view next to pseudocode](docs/images/graph.png)

- **Type editors**: a Local types editor with a C declaration editor (highlighting, parser errors on the line), structure/union and enum tables, and the stack frame of a function.

  ![Local types editor](docs/images/type-editor.png)

- **IDAPython and IDC console** in the bottom panel: multi-line input, history, coloured output and tracebacks, a Python / IDC switch, Cancel for a running script (and "Kill host" when it ignores Cancel), and `File > Script file...`.

  ![Python console](docs/images/python-console.png)

- **Proximity browser**: the call neighbourhood of a function, callers above and callees below, with a minimap.

  ![Proximity browser](docs/images/proximity.png)

- **Command palette and jump-anywhere**: every command is searchable, and `G` jumps to an address, name or expression.

  ![Command palette](docs/images/palette.png)

- **FLIRT signatures**, **Lumina** pull and (confirmed) push, and **plugins**: IDA's plugin loader entries are listed, can be run headless, and get keymap entries.
- **Debugger** (Windows, user mode): start or attach, breakpoints (conditional, hardware), step into / over / out, run to cursor, registers (editable, changes highlighted), stack, memory, call stack, threads, modules, exceptions and the program's debug output, with markers in the listing, graph, pseudocode and hex view. Details below.
- **Several databases per window**, one host process each, with analysis progress, safe close (Save / Don't save / Cancel) and recent files.
- **Settings**: themes (Dark, Light, High contrast, IDA Classic), fonts, an editable keymap with conflict detection, analysis, debugger and Python options, and an About section with the IDA and SDK versions and paths.

## Requirements

- Windows 10 or 11 (x64).
- **IDA Pro with idalib** and a valid licence. The Hex-Rays decompiler is needed for pseudocode. ida-zui looks for the newest `IDA Professional *`, `IDA Pro *` or `IDA *` folder under `%ProgramFiles%` that contains `idalib.dll`, or uses `IDADIR`.
- The **IDA SDK** matching your IDA, to build the host. Extract it to `third_party/idasdk*/` (for example `third_party/idasdk/`) or point `IDASDKDIR` at it. It is licensed by Hex-Rays and git-ignored: never commit it.
- Rust 1.98 or newer (edition 2024) and the MSVC toolchain (Visual Studio 2022 Build Tools with the C++ workload).

## Install (release zip)

```powershell
.\scripts\package.ps1        # builds release, writes dist\ida-zui-<version>-windows-x64.zip
```

The zip holds `ida-zui.exe`, `ida-host.exe` (keep them in one folder), `LICENSE`, `THIRD_PARTY.md` and a short `README.txt`. It needs only IDA Pro with idalib installed on the machine; the SDK is a build-time dependency. Extract anywhere and run `ida-zui.exe`.

On first start ida-zui looks for IDA under `%ProgramFiles%` (see Requirements). If it finds none, the welcome screen says so and offers **Locate IDA...** (also `File > Locate IDA...`): pick the folder that contains `idalib.dll` and the choice is saved in `settings.json`. `IDADIR` always wins over the saved folder. If `ida-host.exe` is missing next to `ida-zui.exe`, the welcome screen names the path it expected. If IDA refuses to start (licence, broken install), its own message is shown on the failed tab.

## Quick start

```powershell
git clone https://github.com/heyimada/ida-zui.git
cd ida-zui
# Extract the IDA SDK into third_party\idasdk (or set $env:IDASDKDIR)
cargo build --workspace --bins          # builds ida-zui.exe and ida-host.exe
.\target\debug\ida-zui.exe C:\path\to\binary.exe [--goto main]
```

`ida-zui.exe` looks for `ida-host.exe` next to itself. Opening a binary creates or reuses `<binary>.i64` beside it, as IDA does. Drag and drop, `File > Open` and `ida-zui a.exe b.dll` all work. Use `--release` for an optimised build.

Without the SDK you can still build and test the UI side: `cargo build -p ida-zui` and `cargo test -p ida-proto -p zui-editor -p ida-zui`. That is what CI does.

## Coming from IDA

The defaults follow IDA's `cfg/idagui.cfg` where IDA has a key. Every key is editable (Settings > Keyboard) and every command is in the menus and the palette (`Ctrl+Shift+P`). The same table is in the app under `Help > Coming from IDA`.

| Action | IDA | ida-zui |
|---|---|---|
| Jump to address / name | `G` | `G` (fuzzy over functions and names) |
| Jump to function / segment / entry point | `Ctrl+P` / `Ctrl+S` / `Ctrl+E` | `Ctrl+P` / `Ctrl+Alt+S` / `Ctrl+E` (`Ctrl+S` saves here) |
| Next / previous function | `Ctrl+Down` / `Ctrl+Up` | same |
| Back / forward | `Esc` / `Ctrl+Enter` | same (plus `Alt+Left` / `Alt+Right`) |
| Follow | `Enter` | `Enter`, double-click (on a mnemonic it follows the first name of the line) |
| Xrefs to / from | `X`, `Ctrl+X` / `Ctrl+J` | same (plus `Shift+X` for from) |
| Rename | `N` | `N` |
| Comment / repeatable comment | `:` / `;` | same (`:` works on any keyboard layout; `/` in pseudocode too) |
| Graph / linear | `Space` | `Space` |
| Pseudocode | `F5`, `Tab` | `F5`, `Tab` (`Tab` again returns) |
| Search text / bytes, again | `Alt+T` / `Alt+B`, `Ctrl+T` / `Ctrl+B` | same |
| Search immediate value, again | `Alt+I`, `Ctrl+I` | same |
| Mark position / jump to marked position | `Alt+M` / `Ctrl+M` | same (a ribbon in the listing gutter; `Del` in the chooser removes a mark) |
| Hide casts | `\` | `\` in pseudocode, also `View > Hide casts` (per database) |
| Hex edit mode | `F2` | `F2` in the hex view: type hex digits, `Enter` or `F2` applies as a patch, `Esc` cancels |
| Next code / data / unexplored | `Alt+C` / `Ctrl+D` / `Ctrl+U` | same |
| Next code outside a function | `Alt+U` | `Alt+U` |
| Functions / Names / Strings / Segments | `Shift+F3` / `Shift+F4` / `Shift+F12` / `Shift+F7` | same (side panel tabs) |
| Local types / Signatures | `Shift+F1` / `Shift+F5` | same |
| Code / data / string / undefine / function | `C` `D` `A` `U` `P` | same |
| Operand: hex/decimal, char, binary, offset, struct offset, enum, stack variable | `H` `R` `B` `O` `T` `M` `K` | same (negate `_`, bitwise not `~`; all under `Edit > Operand types`) |
| Array / string type / edit function | `*` / `Alt+A` / `Alt+P` | same |
| Select lines, copy | `Shift+arrows`, `Ctrl+C` | `Shift+arrows`, `Shift+click`, drag, `Ctrl+C` (`Esc` clears; with no selection `Esc` is back) |
| Context menu | right click | right click, `Menu` key or `Ctrl+Alt+M` at the caret |
| Save database | `Ctrl+W` | `Ctrl+W` (`Ctrl+S` too); close database is `Ctrl+Shift+W` |
| Undo / redo | `Ctrl+Z` / `Ctrl+Shift+Z` | same (`Ctrl+Y` too) |
| Debugger | `F9` `F7` `F8` `F2` `Ctrl+F2` `F4` | same |

Differences: the side lists live in a rail on the left instead of floating windows; because `Ctrl+B` is IDA's "search bytes again", the side-panel filter moved to `Ctrl+Shift+B`; `Ctrl+W` saves as in IDA (`Ctrl+S` too) and closing the database is `Ctrl+Shift+W`; a database that was just analysed counts as unsaved; there is no database snapshot manager; plugins that need IDA's Qt UI report that they cannot run headless; the debugger is Windows user mode only.

## Architecture

```
┌──────────────────────────┐   JSON lines over stdio   ┌──────────────────────────────┐
│ ida-zui.exe (crates/app) │ ───────────────────────▶  │ ida-host.exe (crates/ida-host)│
│  zui UI, forbid(unsafe)  │ ◀─────────────────────── │  Rust loop + C++ shim (SDK)   │
│  depends on ida-proto    │   stderr = IDA's output   │  links ida.dll / idalib.dll   │
└──────────────────────────┘                           └──────────────────────────────┘
```

- **`crates/ida-proto`** holds the typed request/reply protocol, the `Client`,
  and the parser for IDA's colour-tagged lines (`tags.rs`). It is pure Rust
  with no IDA dependency, and it is the only crate the UI links.
- **`crates/ida-host`** owns idalib. idalib is single-threaded, so every call
  runs on the host's main thread. `shim/shim.cpp` is a thin `extern "C"` layer
  over the SDK that returns JSON. IDA's own stdout is redirected to stderr, and
  the UI shows it in the output panel. Besides replies, the host writes
  unsolicited `{"event": ...}` lines: `log` (IDA's messages, captured with a
  `ui_msg` hook), `db_changed` (IDB hooks, coalesced to at most ~8 per second,
  tagged `request` or `python` by origin) and `analysis` (stage, percent,
  functions found). `ida_proto::Client::spawn_with_events` routes them to a
  channel per host.
- **`crates/app`** is the UI.

The host runs in its own process for three reasons:

1. `ida.dll` exports data symbols (`callui`), so it can't be delay-loaded.
2. An IDA fatal error then can't take the UI down with it.
3. idalib allows one database per process.

The host exits when its stdin closes or when the UI process dies.

## Window layout

Top to bottom: title strip (the product glyph, then one chip per open database), menu bar, a 44 px toolbar, the body and a status bar. The toolbar has the sidebar toggle, back, forward and save (a dot marks unsaved changes), the file pill, a **Linear | Graph | Hex | Pseudocode** control, a split toggle (listing or graph beside pseudocode), the jump field and the bottom-panel and inspector toggles. The body is an activity rail (Functions, Names, Strings, Imports, Exports, Segments, Types), the list, the editor column and the inspector.

- **Editor column:** a tab strip (the main view, then Local types, structures, enums and frames), a jump bar (`file > segment > function > address`), the centre view (or two panes, each with its own tab strip; `Tab` / `F5` move the keyboard between them) and the bottom panel.
- **Inspector:** Info (function facts, prototype, xrefs, callees; in graph view the basic block, its successors, predecessors and calls), Type, Xrefs and Comments.
- **Bottom panel:** Output, live Xrefs to the name under the caret (click a row to jump) and a Python tab with the IDAPython console (below).
- Panels toggle from the toolbar and the View menu (`Ctrl+Alt+I` inspector, `Ctrl+Alt+J` bottom panel) and drag to resize; sizes, visibility and the split are remembered in `settings.json`.
- The graph has a header strip (zoom controls, edge legend, minimap toggle) and a bottom-right minimap (click to pan; `View > Graph > Minimap`). Blocks drop the indent their instructions share. The palette groups results with kind badges and highlights the matched text. The themes are Dark and Light (plus High contrast and IDA Classic); IBM Plex Sans is bundled.

## Debugger

`Debugger` menu, toolbar controls while a process exists, and panels in the bottom dock (`Debugger > Debugger windows`): Registers, Stack, Memory, Call stack, Threads, Modules, Breakpoints, Events and Exceptions. It drives IDA's own `win32` debugger module through the host, so conditions, hardware breakpoints and the exception table are IDA's.

- **Start / attach:** `F9` starts the database's input file (or the program, arguments and folder from `Settings > Debugger`) and continues a suspended process. `Debugger > Attach to process` lists running processes. Stopping at the entry point, on library or thread events and disabling ASLR are options; the exception policy (IDA defaults, pass all, stop on all) is applied at start and the Exceptions panel sets single codes.
- **Breakpoints:** `F2` or a click in the marker gutter (listing, graph, pseudocode) sets one; a filled red dot is enabled, an outline is disabled. The Breakpoints panel enables, removes, shows hit counts (this session) and edits the condition (double-click; an IDC expression such as `RCX == 3`, or `py: ida_dbg.get_reg_val('RCX') == 3` for Python). They are stored in the database.
- **Stops:** the instruction pointer shows as an arrow and a highlighted line in the listing, graph, pseudocode and hex view, and the views follow it. The status bar says what happened (breakpoint, step, exception). Registers that changed since the last stop are highlighted; click a value to edit it (an expression is accepted), click a flag to toggle it.
- **Rebasing:** the system loads the program at an address of its own choosing, so the database is rebased to it while the process lives and rebased back when it ends. Saving (or closing with save) during a session ends the session first, and a rebased database is never written. Every address the UI holds (history, cursor, lists) follows the move.
- **Safety:** the debugger runs real code. Debug only programs you trust. The tests debug a small program compiled from `scripts/debuggee/debuggee.c`; symbol servers are never contacted (`DOPT_LOAD_DINFO` stays off).

## Plugins and scrollbars

- **Plugins** (`Edit > Plugins`, `Windows > Plugins`, palette "Run plugin: <name>"): the host lists IDA's plugin loader entries (name, hotkey, flags, path, loaded state, from IDADIR and every IDAUSR plugins folder) and runs one by path with an argument (`plugin_list`, `plugin_run`). Each runnable plugin is a keymap command in the "Plugins" category with its own hotkey as default (editable under Settings > Keyboard). The Plugins window filters, shows hidden entries, runs the selection and opens the plugins folder. A plugin that needs IDA's Qt UI is reported as unable to run headless with its log, never hangs. Tests never run your own plugins.
- **Scrollbars**: an overlay bar (`scrollbar.rs`) on listing, pseudocode, hex, side lists, palette and modal lists, bottom panel, inspector, settings, tables, editors and both graph canvases (both axes against the graph bounds): thumb appears on hover, scroll and drag; track click pages; thumb drag scrolls.

## Signatures, Lumina and the proximity browser

- **Signatures** (`Windows > Signatures`, `Shift+F5`, palette): a document tab with the FLIRT signatures in the database's list (name, description, state, modules matched) and the `.sig` files available for its processor, with their description and function count. Click rows to pick files (applied ones are greyed), filter by name or description, press **Apply**. The analysis queues run again with progress in the status bar, the lists refresh and library functions get the `lib` badge.
- **Lumina** (`File > Lumina`, palette): pull metadata for all functions or the current one, and push (with a confirmation). IDA has no SDK API for Lumina, so the actions are the kernel's own (`LuminaPullAllMds`, `LuminaPullMd`, ...) and their outcome is read from IDA's log and the number of functions that carry Lumina metadata; an unreachable server shows a dialog with IDA's message. **A push uploads your analysis to a third-party server.** The UI asks first, naming the server and what is uploaded, with Cancel as the default answer; the host refuses a push without the confirmation flag, and always when `IDA_ZUI_AUTOMATION` or `IZS_BLOCK_LUMINA_PUSH` is set (the E2E and capture scripts never push). The server comes from IDA's own setting and is never changed.
- **Proximity browser** (`Windows > Proximity browser`, palette): the call neighbourhood of the function at the cursor as a document tab "Proximity: <name>", callers above and callees below (2 levels each; steppers in the header), at most 12 neighbours per node and 150 nodes. Labels use the code font at its normal size and scale with the zoom; the view fits on open; a level wraps into rows of 4 and folds past 8 nodes into a "+N more" chip (click it to open the level). A node shows its address and call counts and a + / - button when it has more callers or callees than shown. Click a node to recentre on it, double-click to jump to it in the listing; drag to pan, `Ctrl+wheel` to zoom, minimap as in the graph.

## Python console and scripts

The bottom panel's **Python** tab (`View > Open subviews > Python console`) is an IDAPython REPL running in the database's host. The input is the multi-line code editor with the Python highlighter: **Enter** runs the statement once it is complete (Python's own `codeop` decides, so an open block or bracket continues on the next line and a block ends with a blank line, as in IDA's console), **Shift+Enter** adds a line, **Ctrl+Enter** runs whatever is there, **Up / Down** on the first / last line walk the history (the half-typed text comes back). Statements run in `__main__`, the value of a trailing expression is printed, and `idc.here()` / `ida_kernwin.get_screen_ea()` are the address under the UI cursor. Output is coloured by stream (stdout, stderr, expression values, tracebacks, which show the source line). History (500 entries, adjustable in Settings > Python) is kept in `python_history.json` next to `settings.json`.

While a script runs the host cannot answer other requests, so the console shows a *Running* strip with elapsed time and **Cancel**, which raises `KeyboardInterrupt` in the script (a script stuck in a long C call or `time.sleep` stops when that call returns; about 4 s after Cancel a **Kill host...** button appears, which asks twice because unsaved changes are lost). A script's changes reach the views through `db_changed` events once it returns; edits made from Python show in the listing, lists and inspector without a manual refresh. One undo reverts a whole script run. `File > Script file...` (`Alt+F7`) runs a `.py` file. The **Python / IDC** button in the panel header switches the input language: IDC runs through IDA's own evaluator (an expression prints its value; statements such as `auto x = 5; Message("hi");` run as a snippet; errors come back as `IDC: ...`).

## Analysis progress

Opening a binary steps the auto-analysis queues instead of waiting in one call. The loading screen shows a progress bar with the queue's stage, a percentage (an estimate from the stage and the address, never going backwards), functions found so far and elapsed time; the database chip and the status bar show the percentage too, and edits that re-run the queues (make code, create function) report the same way. Turn it off, or analysis on open, in Settings > Analysis. Timing against IDA's own loop is in `docs/PLAN.md`.

## Several databases

Every open database is a chip in the title strip: file name, a spinner and elapsed time while it analyses, an unsaved dot and a close button. Each has its own host process, session, views, editor tabs, history and panel selections; the menu bar, settings, keymap and panel sizes are shared. Opening a file (File > Open, drag and drop, recent files, `ida-zui a.exe b.dll`) adds a chip, and opening one that is already open switches to it. `Ctrl+Tab` / `Ctrl+Shift+Tab` and `Ctrl+1..9` switch, `Ctrl+Shift+W` / the chip's X close one (asking Save / Don't save / Cancel for that database), and the Windows menu lists them. Closing the window asks once for all unsaved databases, then closes every host. Tabs in the background neither repaint the window nor poll.

## Menus, settings and keys

The window has a menu bar (File, Edit, Jump, Search, View, Options, Windows,
Help; the Debugger menu is described under Debugger). Click a title, hover to
switch menus, use the arrow keys and Enter, press `Alt` or `F10` to focus the
bar, or `Alt+F/E/J/S/V/O/W/H` to open a menu. Shortcuts shown in the menus come
from the live keymap. Every command is also in the command palette. The menus
follow IDA's split: **View** holds the main views, `Open subviews` (every list
and editor once), `Toolbars and panels` and `Graph`; **Windows** holds the
database tabs, `Close editor tab` and `Reset layout` (panel sizes, visibility
and the split back to their defaults); **Jump** and **Search** carry the
choosers and searches above. Underlined letters are menu mnemonics (a label can
mark its own with `&` in `menubar.rs`). `Edit` also has `Operand types` (the current representation is checked), `Array…`, `String type…`, `Edit function…` and `Patch program` (change bytes, assemble, revert patches, the list of patched bytes, and `Apply patches to input file…`, which writes a copy named `<input>.patched` by default and never overwrites the input). `Help` has `Documentation` and
`Report an issue` (they open the GitHub pages in your browser), `Coming from
IDA`, the shortcut list and `About`.

The side lists (Functions, Names, Strings, Imports, Exports) are tables with IDA's columns: click a header to sort (numbers by value, names ignoring case; again to reverse), drag a column edge to resize, and the sort and widths are remembered. The Segments strip under them collapses. Comments (`:` / `;`) open a multi-line editor (`Enter` is a new line, `Ctrl+Enter` applies). The hex view has an offset header, a Group control (1, 2, 4, 8 bytes) and byte editing with `F2`; patched bytes show in the warning colour. Right-click menus also exist on the Output and Python panels (Copy, Clear) and in the type editors (Cut, Copy, Paste, Select all, Apply).

`Options > Settings...` (`Ctrl+,`) holds the theme (Dark, Light, High contrast,
IDA Classic), the code font and size, the UI font size and the keyboard
shortcut editor (click a command, press the new keys; conflicts are flagged).
Settings live in `%APPDATA%\ida-zui\settings.json`; a missing or damaged file
means defaults. `File > Open with options...` (`Ctrl+Shift+O`) opens a raw file
with the Binary loader, a processor (x86, ARM/AArch64, MIPS, PowerPC, 68k, 8051,
AVR, RISC-V, SH4, Z80, WebAssembly), a base address and optional auto-analysis;
the dialog says where the copy and the database will be written. Recent files
(with a processor badge) show on the welcome screen and in `File > Recent files`. `Help > About` shows the
IDA and SDK versions and paths.

## Keyboard shortcuts

IDA's defaults are used where IDA has one.

| Key | Action |
|---|---|
| `G` | Jump to an address, name or expression (fuzzy over functions and names) |
| `Ctrl+P` / `Ctrl+Alt+S` / `Ctrl+E` | Jump to function / segment (choosers) / entry point |
| `Ctrl+Down` / `Ctrl+Up` | Next / previous function |
| `Alt+I` / `Ctrl+I` | Search an immediate value (a number), then again |
| `Alt+C` / `Ctrl+D` / `Ctrl+U` / `Alt+U` | Next code / data / unexplored / code outside a function |
| `Esc` / `Alt+←`, `Ctrl+Enter` / `Alt+→` | Back, forward |
| `Enter`, double-click | Follow the name under the caret, or expand a collapsed item |
| `Space` | Toggle between linear and graph view |
| `Tab` / `F5` | Show pseudocode in the main view, `Tab` again returns to the view you left (with the split on: switch focus between the panes). The layout is not saved; only the split toggle is |
| `Ctrl+Shift+H` / `Ctrl+Shift+D` | Hex view / disassembly |
| `N` | Rename (a local variable in pseudocode) |
| `:` / `;` | Comment / repeatable comment (IDA's assignment) |
| `/` (pseudocode) | Comment (Hex-Rays' key) |
| `Y` | Set the type (a function prototype, or a local variable's type in pseudocode) |
| `X` / `Shift+X` | Cross references to / from |
| `C` `D` `A` `U` `P` | Make code, cycle data size, make string, undefine, create function |
| `Shift+Del` | Delete function |
| `-` | Expand or collapse a function or hidden range |
| `Ctrl+W` (or `Ctrl+S`), `Ctrl+Z`, `Ctrl+Y` | Save, undo, redo |
| `Ctrl+Shift+W` | Close the database |
| `Shift+Up/Down/PgUp/PgDn/Home/End`, `Shift+click`, drag (also past the top or bottom edge, which scrolls) | Select lines (listing, graph block, pseudocode) or bytes (hex); `Esc` clears the selection |
| `Ctrl+C` | Copy the selected lines, or the current one (hex: the bytes as `48 8B C4`); `Edit > Copy with addresses` keeps the address prefix, the hex view also copies as a C array or Python bytes, and `Ctrl+Shift+C` copies the address |
| Right click, `Menu` key or `Ctrl+Alt+M` (`Shift+F10` stays IDA's Enums) | Context menu of the line, token, byte or list row under the caret (the same commands as the menu bar, with their shortcuts) |
| `H` / `R` / `B` / `O` | Operand: hex or decimal / character / binary / offset |
| `T` / `M` / `K` | Operand: structure offset / enum member / stack variable (choosers) |
| `_` / `~` | Operand: negate / bitwise not |
| `*` / `Alt+A` / `Alt+P` | Array dialog / string type chooser / edit function dialog |
| `Ctrl+Shift+B` | Filter the side panel |
| `Shift+F1` / `Shift+F9` / `Shift+F10` | Local types editor / pick a structure / pick an enum |
| `Ctrl+X` / `Ctrl+J` | Xrefs to / from (IDA's keys; `X`, `Shift+X` also work) |
| `Alt+T` / `Ctrl+T`, `Alt+B` / `Ctrl+B` | Search text / bytes, then search again (the last pattern is prefilled) |
| `Shift+F3`, `Shift+F4`, `Shift+F12`, `Shift+F7` | Functions, Names, Strings, Segments |
| `Ctrl+K` | Stack frame of the function at the cursor |
| `Ctrl+F4` | Close the editor tab |
| In the structure, enum and frame tables: `N` `Y` `;` `D` `U`/`Del` `X` `Ins` | Rename, retype (value for enums), comment, cycle size / define, delete, xrefs, add |
| In the type editor: `Ctrl+S` / `Ctrl+Enter` | Apply the declaration |
| `` Ctrl+` `` | Output panel (IDA's messages) |
| `Alt+F7` | Run a Python script file |
| In the Python console: `Enter` / `Shift+Enter` / `Ctrl+Enter` / `Up`, `Down` / `Tab` | Run if complete / new line / run anyway / history / complete the name before the caret (again to cycle) |
| `Ctrl+Alt+J` / `Ctrl+Alt+I` | Show or hide the bottom panel / the inspector |
| `F9` / `F12` / `Ctrl+F2` | Debugger: start or continue / pause / terminate |
| `F7` / `F8` / `Ctrl+F7` / `F4` | Debugger: step into / step over / run until return / run to cursor |
| `F2` / `Ctrl+Alt+B` | Debugger: toggle a breakpoint at the cursor / breakpoint list |
| `Ctrl+Shift+P` | Command palette (every command, searchable) |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | Graph zoom (`Ctrl+wheel` also zooms; drag to pan) |

## Testing

- **Unit tests** (`cargo test --workspace`) cover pure logic: the colour-tag parser, the graph layout invariants, the declaration-error mapper, the editor's text model and scrollbar geometry.
- **Protocol E2E** (`.\scripts\e2e.ps1 [-Sample <exe>]`) builds everything, opens a fresh copy of the sample (default `C:\Windows\System32\find.exe`) through `ida-host.exe` and exercises every request plus edit, undo/redo, save and reopen, then stops an analysis in progress on a copy of `shell32.dll` (`stop_e2e`: the stop is answered at once, the host closes and leaves no database files) and runs the debugger E2E. It writes a deterministic dump to `artifacts/e2e/<name>.txt` and IDA's log next to it, and exits non-zero if any check fails. It needs your IDA and SDK.
- **UI automation** (`.\scripts\ui-capture.ps1 -Goto <expr> -Steps "wait:15000","shot:x","key:tab","action:ida::OpenSettings",...`) drives the real app through its in-app automation channel (`IDA_ZUI_AUTOMATION`): no OS input is synthesised and only the app window is captured, so it never touches your other windows or clipboard. Steps: `key:`, `type:`, `action:`, `win:n` (aim at floating window n), `click:x,y`, `confirm:n`, `drop:path`, `marker:`, `shot:`, `close`, `waitexit:ms`, `files:glob`, `procs` and `hostsgone` (fails the script if a host outlives its tab); switches `-NoFile`, `-Reuse`, `-Settings '<json>'`, `-Release`, `-Log name`. Screenshots go to `artifacts/ui/*.png`. The screenshots in this README were made this way on `find.exe`.

Tests never push to Lumina: the host refuses a push whenever `IDA_ZUI_AUTOMATION` or `IZS_BLOCK_LUMINA_PUSH` is set.

## Status and known gaps

**Works**
- Linear disassembly with IDA's own line generator: colours, comments, xref
  annotations. It pages in both directions and expands collapsed items.
- Hex-Rays pseudocode with anchor-based navigation, local-variable rename and
  retype, and cursor sync with the listing.
- Graph view with a layered layout, taken / not-taken / back edges, zoom and pan.
- Hex view over segments.
- Side lists for functions, names, strings, imports, exports, segments and
  local types, all filterable. Double-clicking a type opens its editor.
- Type editors, in tabs next to Disassembly / Graph / Hex (View and Windows
  menus, palette, Types side tab): a **Local types** editor with a list and a
  C declaration editor (highlighting, parser errors as markers on the line, new
  type, delete with dependents, header import by paste); a **structure / union**
  table with inline editing, add / delete / retype, member xrefs and a C
  declaration mode; an **enum** table (hex / decimal, bit flags); and the
  **stack frame** of a function (locals, saved registers, return address,
  arguments, undefined gaps you can define). Edits are undoable and show in the
  listing and pseudocode.
- Jump-anywhere, xrefs, command palette.
- Rename, comments, types, code/data/string/function editing, undo/redo, save.
- Closing is safe: every way of closing the window (Alt+F4, the title-bar X,
  Ctrl+Q) asks Save / Don't save / Cancel when there are unsaved changes, then
  closes the database through the host, which packs it into the `.i64`. A
  database that was freshly analyzed counts as unsaved. Closing one database tab asks the same question for that database only. Closing a tab or the window while an analysis is running asks "Stop analysis and close" or "Cancel"; the host stops between two analysis steps (`analysis_stop`) and closes without leaving database files behind. A host that does not
  answer within 15 s is killed as a last resort. The listing columns ida-zui sets are put back to IDA's own before every save, so they never reach the `.i64`.
- Large databases work: `shell32.dll` (34k functions, 55k names) opens and pages
  through the listing in both directions; the 32-bit `find.exe` works too.

**Partial**
- **Type editors:** `parse_decls` reports no line numbers. When a text with several declarations fails, each declaration is parsed alone and its messages are placed inside its own lines; within one declaration the error is still found by the token IDA names. Struct-member xrefs are covered by the protocol E2E with a struct offset applied to a real operand (IDA adds the member xref when the analysis queue processes it). IME input in the code editor is untested.
- **Text search** uses IDA's `find_text`, so it can be slow on large databases.
- **Refresh after edits:** edits made in this UI refresh through their own request; changes made by scripts refresh through `db_changed` events once the script returns (not while it runs, since the host is busy). Plugins that change the database outside a request would be reported with origin `request` and are not refreshed.
- **Python console:** Tab completes dotted names through Python's `rlcompleter` (no inline help, nothing for IDC); Cancel needs the script to be running Python bytecode, otherwise use Kill host (see above). IDC's `Message()` output goes through IDA's log, which the console shows as it arrives. Output streams into the console while a script runs, as plain lines (IDA's own messages included); when the script returns they are replaced by the same output split by stream (stdout, stderr, values, tracebacks).
- **Several databases:** tab switching and chip clicks were driven through the automation channel, which bypasses the OS hit test, so a real mouse click on a chip (outside the drag area) is untested. Opening a file never replaces an open database (it gets a tab of its own), so there is no open-over-unsaved prompt; a drop is simulated through gpui's file-drop events.
- **Analysis percentage** is an estimate (stage bands calibrated on shell32.dll), not IDA's own measure.
- **Graph edges** are routed around blocks: a long edge gets a waypoint in every layer it crosses and runs in the gaps between layers, loops use lanes right of the graph. Two edges may still share a short stretch of line where waypoints of different layers coincide, and the crossing count is only reduced (barycentre ordering), not minimised.
- **Lumina:** pull and push are wired to the kernel's own actions and read their outcome from IDA's log. Only the error path (an unreachable server) has been exercised; a successful pull against a live server has not been verified. Push is never executed by any test (see above).
- **Proximity:** the + buttons (callers and callees), the Data toggle and double-click navigation were driven by automation clicks. After an expansion the view pans and zooms out to keep the new nodes in sight; with Data on, data targets get their own share of the fan-out and show as nodes with the data colour.
- **Signatures:** applying runs the whole analysis queue again, as IDA does; the window lists `.sig` files of the processor's `sig` directories in `IDADIR` and `IDAUSR` and reads their title and function count from the file header. Un-applying a signature is not offered (IDA cannot remove one from a database either).

**Restyle gaps**
- Scrollbars cover every scrollable surface except the single-line Python input and menu bar dropdowns that fit. The editor's scrollbar path (`editor_source`) was checked only through the same code as the other surfaces: no screenshot of an editor with overflowing text was taken. The listing's thumb reflects the loaded buffer, not the whole address space.
- Graph edges: loops and long edges into one block share one trunk and one arrival port. Edges sharing only a source are not bundled. The graph zooms out to 3% (blocks are plain boxes below 30%) so a large function fits.
- The focus ring is a 2 px spread without the 1 px offset.
- The split is off by default (the design's main window shows it on).
- High contrast and IDA Classic have no mockup. They were reviewed by screenshot, and every text token reaches 4.5:1 on every surface it can sit on (a unit test checks this).

**Debugger gaps**
- The listing shows the database's bytes, not the process memory, so self-modifying or unpacked code shows as analysed there. The hex view can show the suspended process instead (`Debugger > Hex view: process memory`; a "PROCESS MEMORY" badge marks it, and it is re-read at every stop); the Memory panel always reads the process.
- First or last chance of an exception is not reported by IDA's API; it is inferred (the same exception at the same address right after a continue is the last chance).
- Hit counts are the UI session's own count of stops (IDA's API gives none). The pseudocode marker sits on the line of the closest ctree item at or before the instruction pointer: exact whenever Hex-Rays has an item at that address, the enclosing statement otherwise (docs/GAPS.md D7, D8).
- Pausing uses a request that a target may never honour; the UI then offers to terminate (`pause_timeout`).
- Attach and detach were tested against a process we start ourselves.

**Non-goals for now**
- Debugging is Windows user mode only: no remote, Linux or kernel debugging, no source-level debugging, no tracing, no appcall.

The full list of known gaps, with priorities and suggested fixes, is in [docs/GAPS.md](docs/GAPS.md). The roadmap is [docs/PLAN.md](docs/PLAN.md), and technical notes on idalib are in [docs/IDALIB_FINDINGS.md](docs/IDALIB_FINDINGS.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) (the contributor and coding-agent guide). Third-party components are listed in [THIRD_PARTY.md](THIRD_PARTY.md).

## License

ida-zui is released under the [MIT License](LICENSE).

**Disclaimer.** ida-zui is not affiliated with, endorsed by or sponsored by Hex-Rays. IDA and IDA Pro are trademarks of Hex-Rays SA. This repository contains no IDA binaries, databases or SDK files: you need your own IDA licence and SDK.
