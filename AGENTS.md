# Working on ida-zui (read this first)

ida-zui replaces the IDA Pro Qt UI with a Rust UI built on **zui**, the zeronsh fork of Zed's gpui, pinned at rev `1561532`. IDA runs headless through idalib in a separate process.

## Layout

| Path | What lives there |
|---|---|
| `crates/ida-proto` | Pure Rust. `lib.rs` declares every request with the `methods!` macro (name, params, reply type). `client.rs` spawns the host and multiplexes requests. `tags.rs` parses IDA color-tagged lines and is unit-tested. `examples/e2e.rs` is the protocol E2E. |
| `crates/ida-host` | `ida-host.exe`. `src/main.rs` reads JSON requests on stdin and dispatches them to `shim/shim.cpp` through `extern "C"` `izs_*` functions. Each function returns 0 and leaves JSON in the result buffer, or returns non-zero with an error message. `shim/json.hpp` is a tiny JSON writer. `shim/stdio.cpp` redirects IDA's stdout to stderr and holds the Windows-only pieces (thread id, Python interrupt). `shim/events.cpp` is the event stream (`ui_msg`/`ui_screenea` hook, coalesced IDB hooks, flusher thread, stepped analysis loop) and `shim/pyexec.cpp` the Python runner; `shim/edit.cpp` holds the Edit-menu commands (operand representation, arrays, typed strings, function properties, patching, marks); the host's reader thread answers `interrupt` (stops a script) and `analysis_stop` (the stepped analysis loop checks a sticky flag between steps) while the main thread is busy. The build finds the SDK through `IDASDKDIR`, else the newest `third_party/idasdk*/` (git-ignored and licensed: never commit or copy it elsewhere). |
| `crates/app` | `ida-zui.exe`, which has `#![forbid(unsafe_code)]`. |

Files in `crates/app`:

| File | Role |
|---|---|
| `session.rs` | One database: client, navigation history, dirty flag, cached function/name lists. Emits `SessionEvent`; every view subscribes to it. |
| `panes.rs` | Pure tab-strip logic: reorder, which tab follows a close, next/previous. |
| `nav_history.rs` | Pure back/forward history; one per pinned code tab. |
| `workspace/tabs.rs` | Code tabs, the two panes, tab drag, persistence of open tabs per database. |
| `workspace/floating.rs` | `FloatingWindow`: a code view in an OS window of its own over the shared session (own prompts, goto, context menu, status line); docking back on close; `FloatingWindows` registry used by the automation channel's `@<n>` prefix; `Overlay`/`Inbox` carry what the workspace opens (modals, dialogs) for the window the command came from. |
| `workspace.rs` | Root view: layout, modals, global actions. |
| `listing.rs` | Linear disassembly, paged with `listing_up` / `listing_down`. |
| `pseudo.rs` | Hex-Rays pseudocode. Clicks resolve through ctree anchors, not colors. |
| `graph.rs` + `graph_layout.rs` | Graph view; the layout is pure and unit-tested. |
| `hex.rs` | Hex view. |
| `sidebar.rs` | Filterable lists. |
| `modal.rs` | Prompt, picker and palette overlay. |
| `input.rs` | Single-line text input. |
| `code.rs` | Painting and hit-testing tagged lines. |
| `commands.rs` | Shared rename, comment, type, follow and edit helpers. |
| `actions.rs` | The command catalog: every action with label, default keys, search aliases and `needs_db`. Bindings, palette, menus, shortcut editor and help all derive from it. |
| `list_table.rs` | Sort comparators, header-click state and column widths of the side-list tables (pure, unit-tested); `sidebar.rs` draws them and saves them in `settings.json` (`side_tables`). |
| `selection.rs` | Line / byte selection anchor and the text Copy produces (listing, graph block, pseudocode, hex). Pure and unit-tested. |
| `context_menu.rs` | The entries of the right-click / Menu key menus, built from the catalog; `show` asks the workspace to open one through `SessionEvent::ContextMenu` and the menu bar draws it (`MenuBar::open_popup`). |
| `operands.rs`, `edit_dialogs.rs`, `workspace/edit.rs` | Edit > Operand types, Array, String type, Edit function and Patch program: pure helpers (unit-tested), the dialogs, and the workspace commands. |
| `menubar.rs` | In-app menu bar, built from the catalog and the live keymap. A label may mark its mnemonic with `&` (`Make &data`; `&&` is a literal ampersand). |
| `settings.rs`, `settings_view.rs` | `settings.json` (theme, fonts, keymap overrides, recent files) and the Settings dialog with the shortcut editor. |
| `dialog.rs`, `open_dialog.rs`, `help.rs` | In-app dialogs: question, open-with-options, About, shortcut list, Coming from IDA. No native message boxes. |
| `automation.rs` | The automation channel used by `scripts/ui-capture.ps1`. `automation::open_url` is the only way to open a browser: under automation it appends `url: <url>` to `<file>.done` instead. |
| `theme.rs` | The palettes (Dark and Light from the design spec; High contrast, IDA Classic extended to the same token set) and font cells. Views use tokens only, never literal colors. |
| `ui.rs` | Design-system pieces: `icon`, `icon_button`, `badge`, `key_cap`, `button` (Primary/Secondary/Ghost/Danger), `segment`, `panel_header`, tooltips, `ring`/`drop_shadow`. Build new UI from these. |
| `assets.rs`, `assets/` | Embedded icons (`assets/icons/*.svg`, 16 px stroke, tinted by text colour) and IBM Plex Sans (OFL). Adding an icon: drop the SVG in `assets/icons` and run `python scripts/gen_assets.py`. |
| `workspace.rs`, `workspace/chrome.rs` | Root view and its chrome: title strip, toolbar (segmented control, split toggle, jump field), activity rail, tab strip, jump bar, status bar, welcome. Panel sizes and visibility persist in `settings.json` (`layout`). |
| `debugger.rs`, `session/debug.rs`, `bottom/debug.rs`, `workspace/debug.rs` | The debugger: UI model and pure helpers (unit-tested), the session half (host `debug` events, commands), the dock panels (registers, stack, memory, call stack, threads, modules, breakpoints, events, exceptions) and the workspace glue. Margin markers are `code::DebugMarks`. The host half is `crates/ida-host/shim/debug.cpp`; `debug_e2e` (run by `scripts/e2e.ps1`) debugs the program built from `scripts/debuggee` and must never be pointed at anything else. |
| `plugins.rs`, `plugins_view.rs` | Plugin list shared by menu, palette, keymap and the Plugins window; running a plugin and its outcome message. |
| `scrollbar.rs` | Overlay scrollbar: `scrollbar(id, Source, Axes, ..)` as the last child of a `relative` parent. Sources: `handle`, `list`, `custom`, `pan_source` (graph canvases), `editor_source`. Painted in prepaint-created hitbox. |
| `focus.rs` | What the caret is on (function summary, xrefs, comments, type), fetched once, debounced, shared by the inspector and the bottom panel. Views publish the caret through `Session::set_caret`. |
| `inspector.rs`, `bottom.rs` | Right inspector (Info / Type / Xrefs / Comments; block info in graph mode) and bottom panel (Output / live Xrefs / Python console). |
| `decl_names.rs` | Which types a C declaration text defines (the Local types header). |

## Clipboard

Every clipboard read and write goes through `zui_editor::clipboard::{read, write}`. Never call `cx.read_from_clipboard` or `cx.write_to_clipboard` directly. When `IDA_ZUI_AUTOMATION` is set, these use an in-process buffer, so a UI test can never read or overwrite the real clipboard. Seed it with the automation command `clipboard <text>` and dump it with `clipdump`. Never read or write the system clipboard, not even to check isolation.

## Adding a host capability (end to end)

1. `crates/ida-proto/src/lib.rs`: add a line to `methods!` and any reply structs (serde).
2. `crates/ida-host/shim/shim.cpp`: add `int izs_xxx(...)` inside `extern "C"`. Start with `REQUIRE_DB()` and wrap the body in `GUARD_BEGIN` / `GUARD_END`. Build JSON with `Json`. Call `undo_point()` before any mutation.
3. `crates/ida-host/src/main.rs`: declare the FFI function and add a dispatch arm.
4. `crates/ida-proto/examples/e2e.rs`: exercise the method against the real binary, assert on the result, and write it into the artifact dump.
5. Only then use it from the UI.

SDK headers are in `<sdk>/include`. Read the real signature in the header before calling anything. idalib has no UI, so `callui`-based helpers (`gen_disasm_text`, `ask_*`, `jumpto`, and so on) may do nothing. Prefer kernel functions.

## zui / gpui gotchas at this rev

- `Entity::update` and `AsyncApp::update` return the closure's value directly. Only `WeakEntity::update` (the `this` in `cx.spawn(async move |this, cx| ..)`) returns a `Result`.
- Edition 2024: a function returning `impl Future` from `&self` must add `+ use<M>`, or it captures `&self`.
- Paint closures (`canvas`) read the entity, clone what they need, paint, then store hit-test data with `entity.update(cx, |v, _| ..)`. Never call `cx.notify()` from paint unless it's a one-shot. `graph.rs`'s `pending_reveal` shows the pattern.
- Keybindings:
  - Single-letter IDA keys are bound only in the `CodeView` key context.
  - Global keys go in `Workspace`.
  - `Modal` and `List` contexts handle Enter, Escape and the arrow keys.
  - `TextInput` has its own context, so typing in inputs never triggers commands.
  - Contributors may use non-US layouts such as German QWERTZ. Avoid bindings that depend on US punctuation positions, and always provide a menu and palette path for every command.
- Use in-app dialogs (`dialog.rs`), not `window.prompt`: native dialogs cannot be automated. `cx.prompt_for_paths` (file picker) only sits behind a Browse button.
- `cx.clear_key_bindings()` wipes everything, so always rebind through `actions::apply_keymap`. Shortcut labels come from `actions::shortcut_text` (keymap level, context-free). Key listeners run after bindings, so the shortcut recorder uses `cx.intercept_keystrokes`.
- A bare Alt press and release arrives as the keystroke `alt` (bound to the menu bar). Dropdowns use `deferred`; a clear sheet under the bar swallows the click that closes them.
- Raw binaries: the host takes `--loader-binary`; the processor is chosen through a private `IDAUSR` (`HostConfig::raw_binary`) plus an input copy named `<file>.izp_<proc>`. An `izp_metapc` map entry breaks every other lookup, so metapc needs no copy. The map holds one entry per line, no commas, and IDA ignores keys longer than nine characters (`processor_extension` shortens long names). Processor names offered in the dialog were verified by opening a blob and reading the processor back.
- The debug build has a console window. The real window title is `<file> — ida-zui`.

## Build, test and verify (all required before you report done)

```powershell
cd <repo root>
cargo build --workspace --bins --examples   # must be warning-free in our crates
cargo test --workspace
.\scripts\e2e.ps1                           # protocol E2E; writes artifacts/e2e/find.exe.txt; exit 0 required
.\scripts\ui-capture.ps1 -Goto "FIND::Initialize" -Steps "wait:16000","keys:...","wait:1500","shot:<name>"
```

Never capture the whole desktop. The `screen` step captures only the app window's rectangle, because a desktop can hold private content.

`ui-capture.ps1` launches the real app on a fresh copy of `find.exe` and drives it through the **automation channel**. With `IDA_ZUI_AUTOMATION=<file>` the app polls the file and runs the appended lines, one per tick, inside its own window: `key <keystroke>`, `type <text>`, `action ida::Name`, `move|click <x> <y>` (logical pixels), `confirm <n>`, `marker <name>` (appends to `<file>.done`), `@<n> <command>` (run in floating window n, 1-based; the script step is `win:<n>`), `clipboard <text>` (seed the in-process clipboard), `clipdump` (append it to `<file>.done`). Also `drop <path>[|<path>]` (files dropped on the window, through gpui's file-drop events). Input goes through gpui's own event dispatch, so nothing reaches the OS and no other window can receive it; the window starts unfocused and is pushed to the bottom of the z-order. There is no SendKeys, SetForegroundWindow or real mouse input anywhere. Mouse steps: `shiftclick:x,y`, `rightclick:x,y`, `drag:x1,y1,x2,y2`. `clipassert:<text>` compares the in-process clipboard with `<text>` (newlines written as backslash-n) and fails the script on a mismatch; `scripts/ui-selection.ps1`, `ui-contextmenu.ps1` and `ui-edit.ps1` are repeatable runs built on them. Script steps: `wait:ms`, `key:ctrl-shift-p`, `type:text`, `action:ida::Rename`, `click:x,y`, `confirm:n`, `marker:name`, `clipboard:text`, `clipdump`, `shot:name` (PrintWindow of the app window only), `frameshot:name` (the same with the OS frame), `nocaption` (fails if Windows draws a title bar above the app's own), `close`, `waitexit:ms`, `files:glob`, `drop:<path>`, `procs` / `hostsgone[:ms]` (remember the app's host processes, then wait for them to exit while the app is still alive; the script exits 1 if one is left). Switches: `-NoFile`, `-Reuse`, `-Settings '<json>'`, `-Release`, `-Keep`, `-Log`. Settings go to `testdata\ui\config` (`IDA_ZUI_CONFIG_DIR`), never your real settings. Pass `-Steps` from PowerShell (bash `-File` does not split the array). Do not `type:` words into a focused listing: single letters are commands (`n` opens Rename); focus an input first. `scripts\p2b-shots.ps1` captures the six design screens in both modes (`artifacts/ui/p2b_*.png`). PNGs land in `artifacts/ui/`; click coordinates are logical, the PNG is physical pixels (`artifacts/ui/scale.txt`). **Open every screenshot you take with the Read tool and look at it.** A feature counts as done only once you have seen it working in a screenshot, or seen it pass an E2E assertion.

## Testing rules

- Prefer E2E tests that produce a verifiable, repeatable artifact.
- Unit tests only for pure logic, written **before** the code. List the failure modes first. No tautological or change-detector tests, and no regression tests unless they close a real behavior gap.
- Report outcomes honestly. If something is partial, say so.

## Other rules

- Never commit anything under `third_party/`, `testdata/` or `artifacts/`, or any IDA binary or database (`.i64`, `.id0`, ...). Do not copy SDK headers into the repo.
- Match the surrounding code: comment density, naming, idioms. Keep each file focused, and create new modules for new views.
- Kill stray processes if you leave any: `Get-Process ida-host,ida-zui -ErrorAction SilentlyContinue | Stop-Process -Force`.
