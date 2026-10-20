# H4: code views as tabs, panes and floating windows

Research only. Nothing here was built or run, and every claim about zui comes from reading its source at rev `1561532`
(`~/.cargo/git/checkouts/zui-05467652636d6f6d/1561532/crates/gpui/src`). Paths below are relative to `crates/app/src`
unless they start with `gpui/`.

## 1. How a code view becomes a tab

### What exists today

- `Database` (`database.rs:47`) owns exactly one `ListingView`, `GraphView`, `HexView` and `PseudoView`, plus a
  `mode: Mode` (`database.rs:21`, Linear / Graph / Hex / Pseudo) that picks which of the four is the **main tab**.
  `Database::doc: Option<u64>` says whether an editor tab is in front instead.
- `DocTab` (`database.rs:29`) holds `{ id, key: DocKey, title, view: DocView, _sub }`. `DocKey` / `DocView` (`docs.rs`) are
  closed enums of editors (types, struct, enum, frame, signatures, plugins, proximity). Opening goes through
  `Workspace::open_doc` (`workspace.rs:682`; it dedupes by `DocKey::same_tab`), closing through `close_doc`
  (`workspace.rs:1109`). The strip is `tab_strip` (`workspace/chrome.rs:831`): a fixed "main" tab (`tab-main`) followed by
  one tab per `DocTab`.
- The centre is `editor_area` (`workspace.rs:1982`). A doc tab in front is rendered full area. Otherwise it renders
  `pane(self.mode)` and, if `Workspace::split` is on, `pane(Mode::Pseudo)` beside it. So a two-pane split already
  exists, but it is a global bool, fixed to "main view | pseudocode", and not part of the tab model.
- `Workspace` derefs to the active `Database` (`workspace.rs:94`), which is why so much code says `self.listing`,
  `self.mode`, `self.doc`.

### Target model

Add a third kind of tab content:

```rust
pub enum TabContent { Code(CodeTab), Doc(DocTab) }
pub struct CodeTab { id: u64, kind: Mode /* rename: CodeKind */, view: CodeView }
pub enum CodeView { Listing(Entity<ListingView>), Graph(Entity<GraphView>), Hex(Entity<HexView>), Pseudo(Entity<PseudoView>) }
```

`Database` then holds `tabs: Vec<TabContent>` and `active_tab: u64`, replacing `listing/graph/hex/pseudo/mode/doc/docs`.
The initial state is one Listing tab (so the app looks the same as today). "New listing / graph / hex / pseudocode tab"
creates a new view entity from the same `Entity<Session>` (`cx.new(|cx| ListingView::new(session.clone(), cx))`, as
`make_db` does at `workspace.rs:175-178`). The Linear/Graph/Hex/Pseudocode segmented control becomes "retarget the
active code tab to that kind" (a new view entity of that kind replaces the content, keeping the tab and its position).
Keep it as a convenience, so the current workflow survives.

Each view already keeps its own cursor, scroll and selection internally (`ListingView::cursor/top`, `PseudoView::cursor`,
`GraphView::cursor`, `HexView`), and only the address is shared through `Session::cursor`. So two listings of one database
are possible without touching the view internals, with the exceptions listed next.

### Places that assume one view per database

1. **Session events filter on a view *kind*, not a view instance.** `Origin` is `{Listing, Graph, Pseudocode, Hex}`
   (`session.rs:21`). Each view ignores `CursorMoved` whose origin equals its own kind
   (`listing.rs:81`, `graph.rs:141`, `pseudo.rs:48`, `hex.rs:44`). With two listings, moving the caret in A would never
   move B, and A would still echo its own move. Fix: `Origin` becomes `{ kind, view_id: u64 }` (or a `ViewId` newtype) and the
   test becomes `origin != self.id`. Whether two views of the same kind should follow each other must then be a
   deliberate choice (default: yes, like IDA's synchronised views; later a "sync off" per-tab toggle).
2. **`Navigated` makes every view jump** (`listing.rs:80`, `graph.rs:140`, `pseudo.rs:47`). Fine for synchronised views.
   An unsynced tab needs a `sync: bool` that skips both events. `Session::navigate/back/forward`
   (`session.rs:522-590`) keep **one** history for the database. That is right for global back/forward but wrong for a
   "per-tab history". Decide up front: one history per database (cheapest, what H4 needs), with per-tab history deferred.
3. **Caret state is one slot.** `Session::caret` (`session.rs:31`, set by `set_caret`, `session.rs:559`) feeds the status
   bar, `Focus` (inspector, live xrefs) and the pseudocode header (`workspace.rs:1962`). With several views it must mean "the
   caret of the view that has focus". Views already publish on caret moves, so the minimum change is: only publish while
   focused, and re-publish on focus-in of a tab. Otherwise a background view moving to a follow-jump would overwrite the
   inspector.
4. **Visibility gating.** `sync_db_views` (`workspace.rs:423`) flips `PseudoView::set_visible`, `GraphView::set_visible`
   and `HexView::visible` from `(mode, doc, split)`. The decompiler and the graph layout only run while visible
   (`pseudo.rs:90`). With tabs this becomes "visible iff this view is rendered in some pane of the front tab". Every view
   needs its own flag set from the layout, not from `Mode`. The pseudocode view re-decompiles on becoming visible, so a
   hidden-then-shown tab is already handled.
5. **The inspector holds the graph.** `Inspector::new(session, focus_ctx, graph, ..)` (`workspace.rs:185`,
   `inspector.rs:52,170,873`) reads `graph.block_info()` when `block_mode`. It must get the *active* graph view (an
   `Option<Entity<GraphView>>` that the workspace updates when the front tab changes), or take block info through
   `Focus`/`Session` instead.
6. **Workspace-level handlers reach into the fixed views.** `set_mode`, `toggle_pseudo`, `main_focus`, `focus_main`,
   `pane`, `editor_area`, `sync_db_views` (`workspace.rs:420-520, 1940-2000`); `w.hex.toggle_process_memory`
   (`workspace.rs:2289`), `w.graph.toggle_minimap` (`workspace.rs:2427`); `ShowGraph/ShowHex/ShowListing/ToggleGraph`
   actions (`workspace.rs:2189-2205`); the segmented control (`chrome.rs:668-690`) and split button. Each becomes "the
   active code tab's view of kind K, if any". Roughly 17 call sites of `set_mode/show_main/show_doc`, plus the ~15 above.
7. **Focus.** `main_focus` returns one focus handle by mode or doc; `toggle_pseudo` (Tab / F5) switches focus between the
   two panes of the split. With tabs it must be "focus of the active tab's active pane".
8. **Keymap context.** Not a problem: all four views use `.key_context("CodeView")` (`listing.rs:728`, `graph.rs:1006`,
   `hex.rs:435`, `pseudo.rs:515`) and register their own action handlers on their element, so any number of them can
   coexist. Single-letter keys stay bound only under `CodeView`. The `Workspace` context and global keys are unaffected.
9. **Automation.** `automation.rs` addresses the one `AnyWindowHandle` it is started with (`automation.rs:35`, `main.rs:131`)
   and runs `key`/`action`/`click` along that window's focus path. There is no view-level step vocabulary, so tabs need
   nothing for existing scripts, but two things are needed for tests of H4: steps to open/switch/close tabs (`action`
   works if each gets an action: `NewListingTab`, `NextTab`, `CloseTab`), and, for tear-off, a way to name a window
   (`window <n>` prefix or "command goes to the window that holds the focus"). Screenshot scripts
   (`scripts/ui-capture.ps1`) address one window too.
10. **Persisted layout.** `settings.layout.split` (`settings.rs:32`) and the last mode are persisted. A tab model needs
    either "restore a single listing" or a small serialisable layout (`[{kind}]`). Per-database tab sets stay unsaved for
    stage (a).
11. **Doc events.** `DocEvent::Navigate` does `show_main()` then `navigate` (`workspace.rs:1101`). "Main" becomes "the most
    recently active code tab" (or a new listing tab if none).
12. **Rebase and close.** `SessionEvent::Rebased` shifts doc tabs that hold addresses (`workspace.rs:289`); code views get it
    through `Session` events like today. `clear_docs` (`workspace.rs:1126`) drops doc tabs on database close; it must keep
    one listing tab alive or recreate it.
13. **Memory/time.** Each `ListingView` fetches rows from the host on `Navigated` and `Changed` events. N listing tabs
    multiply requests (cheap) but also rows held in memory. Background tabs should skip refetching until shown, as
    pseudocode/graph already do.

## 2. Does zui support a second OS window with shared entities?

Yes for the framework; what the app must supply is the glue. Observations from the source:

- **Opening.** `App::open_window(options, build_root_view)` returns a `WindowHandle<V>` (`gpui/app.rs:1217`). It can be called
  from any `App` context at any time, the existing `main.rs:89` does exactly that for the first window, and from an async
  task via `AsyncApp::open_window` (`gpui/app/async_context.rs:186`, which bails when the app is quitting). Each window
  has its own `Window` state and its own root entity.
- **Placement.** `WindowOptions` (`gpui/platform.rs:1768`) has `window_bounds`, `display_id: Option<DisplayId>`
  (`platform.rs:1812`), `kind`, `focus`, `show`, `titlebar`, `is_movable`, `window_min_size`. `App::displays()` /
  `primary_display()` (`gpui/app.rs:1273-1278`) enumerate monitors; `Window::display(cx)` (`gpui/window.rs:2519`) and
  `Window::bounds()` (`gpui/window.rs:2378`) give the current monitor and rectangle, so a tear-off can open on the monitor
  the user dropped it on or on the next one. (Whether a dragged-off position can be read mid-drag is not something I
  checked: simplest is "Move to new window" opening at an offset of the current bounds.)
- **Shared entities.** Entities are owned by the `App`, not by a window, so `Entity<Session>` and friends can be cloned into
  a second window's root. `cx.notify()` on an entity invalidates every window that is currently rendering it
  (`App::notify`, `gpui/app.rs:2522-2544`, filtered through `tracked_entities` per window). `subscribe`/`observe` are
  app-level. A host event therefore repaints all windows with no extra code.
- **Not shareable: one *view* in two windows.** Rendering the same view entity in two windows at once would share its
  hitboxes, scroll handles and measured sizes (the views store paint-time hit data in the entity, e.g. `ListingView::hit`).
  Each window needs its own view entities (as designed in section 1), sharing only `Session`, `Focus` and the like.
- **Focus.** Focus is per window: `Window::focus(&handle, cx)` (`gpui/window.rs:1983`) sets that window's `focus`, and
  `FocusHandle`s themselves live in an app-wide map (`App::focus_handles`, `gpui/app.rs:692`) but are only "focused" relative
  to a window. `window.focus_enabled` exists to suppress focus. Which window is active:
  `Window::is_window_active` (`gpui/window.rs:2426`), `cx.observe_window_activation` (`gpui/app/context.rs:444`),
  `App::active_window` (`gpui/app.rs:1210`), `Window::activate_window` (`gpui/window.rs:5685`).
- **Keybindings.** The `Keymap` is app-global (`gpui/app.rs:693`, `bind_keys` at `gpui/app.rs:2130`, `clear_key_bindings`
  at `gpui/app.rs:2136`; both push `Effect::RefreshWindows`, which dirties all windows, `gpui/app.rs:1704`). Dispatch is
  per window: keystrokes walk that window's `rendered_frame.dispatch_tree` along its focus path
  (`gpui/window.rs:5238-5300`). So bindings are shared and a rebinding via `actions::apply_keymap` applies to all windows,
  while an action handler only fires if the *window that has the key* contains an element with `on_action` for it, or a
  global `cx.on_action` (`gpui/app.rs:2149`, bubble phase only) exists. Consequence: every handler that code views rely on
  from the `Workspace` (Back/Forward, GoTo, Rename dialogs via `Workspace::ask`, the palette, the jump bar, menu
  actions) must also exist in the floating window's root, or be reachable through a global handler. This is the
  biggest piece of work in stage (c).
- **Window lifecycle.** `window.on_window_should_close` is per window (`main.rs:99`), and `cx.on_window_closed`
  (`gpui/app.rs:2275`, used at `main.rs:123`) quits when `cx.windows()` is empty. Both fine for extra windows, but
  `should_close` today asks about every open database and saves (`workspace.rs:1235`), which must only run for the main
  window. A floating window must just release its views.
- **Async replies carry a window handle.** Several places capture `window.window_handle()` and later
  `cx.update_window(handle, ..)` (`workspace.rs:793, 1783`, `workspace/debug.rs:47`, `automation.rs:35`). They are right
  as long as the window they captured is the one that asked; for tear-off they must capture the window of the view that
  started the action.
- **macOS native tabs.** `Window` also has `SystemWindowTabController` hooks (`gpui/window.rs:1405, 1723-1753`): OS-level
  tabbing of windows. Not useful for in-app document tabs and not on Windows.
- **Unverified.** I did not run a second window. The two risks to prove with a 50-line spike before committing to stage (c)
  are text input/IME and the `Modal`/`Dialog` overlays (they are children of `Workspace`, not of a window), and the
  custom title bar (`workspace/chrome.rs` draws its own).

## 3. Staged plan

### Stage (a): code views as tabs (the bulk of the work)

Goal: new Listing/Graph/Hex/Pseudocode tabs, closable, reorderable later; same behaviour with one tab.

Files:
- `database.rs`: `TabContent`, `CodeTab`, `CodeView`; replace `listing/graph/hex/pseudo/mode/doc/docs/next_doc` by
  `tabs`, `active`, `next_tab`. Keep `Mode` as the *kind* enum.
- `docs.rs`: unchanged (`DocKey` stays for editors).
- `session.rs`: `Origin` becomes `{kind, view}` plus helper `is_self(view_id)`; `Caret` gains the origin view. `navigate`
  and history unchanged.
- `listing.rs`, `graph.rs`, `pseudo.rs`, `hex.rs`: a `view_id` field set at construction, event filters switch from kind
  to id, per-view `sync` flag, publish caret only when focused (`set_caret` callers at `listing.rs:509`,
  `graph.rs:437`, `pseudo.rs:195`, `hex.rs`), own `visible` flag.
- `workspace.rs`: `make_db` (one default listing tab), `sync_db_views`, `set_mode` (retarget the active code tab),
  `show_main/show_doc` (-> `activate_tab`), `main_focus`, `toggle_pseudo`, `editor_area`/`pane`, `open_doc`/`close_doc`,
  `clear_docs`, `DocEvent::Navigate`, the ShowX actions, the toggles at 2289/2427, `should_close`, the layout persistence.
- `workspace/chrome.rs`: `tab_strip` renders all tabs (kind icon, title = function at the tab's cursor for listings), close
  buttons, "+" menu; segmented control and split button act on the active tab.
- `inspector.rs`: take the active graph view (`Option<Entity<GraphView>>`, updated on tab change).
- `actions.rs`, `menubar.rs`: `NewListingTab`, `NewGraphTab`, `NewHexTab`, `NewPseudoTab`, `CloseTab`, `NextTab`,
  `PrevTab` with default keys and catalog entries (palette, menu and shortcut editor derive from the catalog).
- `automation.rs`: no code change, new actions are reachable with `action`.
- Tests: pure helper for "which tab becomes active after closing"; an automation capture script for two listings.

Risks:
- `Origin` change touches every view and the status bar; get a view-id round trip right or views echo-loop (A moves ->
  B follows -> B's `set_cursor` -> A follows). `set_cursor` already stops on `self.cursor == ea`, so loops end, but verify.
- Caret/inspector flicker when a background view follows a jump: gate on focus.
- Memory and refetch cost with many listings; background tabs should not refetch until shown.
- Keybinding clash: Ctrl+Tab and Ctrl+W are not bound today; check `actions.rs`. Contributors on QWERTZ: avoid punctuation.
- Decide and document: one navigation history per database (recommended) vs per tab.

Size: about 600-900 changed lines, mostly mechanical. Do the `Origin` and view-id change first and ship it behind the
unchanged single-view UI so it can be reviewed alone.

### Stage (b): split the tab strip into two panes

Goal: drag a tab to the left/right half (or "Split right" in a tab menu) to show two tabs side by side, each pane with its own
strip.

Files:
- `database.rs`: `layout: PaneLayout`, e.g. `Panes { left: Pane, right: Option<Pane>, ratio: f32 }` with
  `Pane { tabs: Vec<TabId>, active: TabId }` (tabs stay in the database's list; panes reference them). Generalising to a
  tree (`Split{axis, a, b}`) costs little more, but two panes is what the audit asks for.
- `workspace.rs`: `editor_area` renders one strip + content per pane with a draggable divider (reuse the sidebar resize
  pattern, `Resize`, `workspace.rs:60`); `focused_pane`; `activate_tab` works within the focused pane; moving a tab between
  panes; visibility per pane. The existing "split with pseudocode" (`Workspace::split`) is subsumed: "Split with pseudocode"
  becomes "open a Pseudocode tab in the other pane"; keep the toolbar button and the F5/Tab behaviour as a shortcut for it, and
  migrate the persisted `layout.split`.
- `workspace/chrome.rs`: `tab_strip` takes a pane; the pane header (`pane_header`) disappears or becomes the strip.
- `scrollbar.rs`: none, but check overlay scrollbars with the narrower panes.
- `settings.rs`: layout schema (ratio).

Risks:
- Focus routing between panes and the `CodeView` key context: both panes have a view in context. The focus path decides, as
  it does for Tab today.
- A tab can be shown in only one pane at once (one entity, one set of hitboxes), so "same tab in both panes" must create a
  second view entity (a "clone view" action) rather than render twice.
- Drag and drop of tabs: gpui drag support exists (`on_drag`/`on_drop`), but a visual drop target preview needs care; start
  with menu items and keyboard actions, add drag later.

Size: about 400-600 lines on top of (a).

### Stage (c): tear off into a new OS window

Goal: "Move to new window" on a code tab; the window can go to another monitor; closing it docks the tab back (or closes it).

Design: do **not** instantiate a second `Workspace`. Introduce `FloatingWindow`, a small root view that owns one pane
(strip optional) plus a status line, holding the `CodeTab` view entities and a clone of `Entity<Session>`. The tab leaves
`Database::tabs` and is stored in the workspace as `floating: Vec<Floating { window: AnyWindowHandle, tab: CodeTab, db: u64 }>`
so the database still knows about it (for close, rebase, save prompts).

Files:
- new `floating.rs`: root view (title bar, content, status), `open(db, tab, cx) -> AnyWindowHandle` via `cx.open_window`
  with `display_id` / bounds from `Window::display/bounds` of the source window; focus handling; closing returns the tab.
- `workspace.rs`: `tear_off_tab`, `dock_tab`, the floating list, `should_close` (also close floating windows), session
  events (a database closing must close its floating windows).
- `actions.rs`/`menubar.rs`: `MoveTabToNewWindow`, `DockTab`.
- Action reachability: either register the needed handlers in `FloatingWindow` (Back/Forward, GoTo, Rename, comment, xrefs,
  jump, palette, copy/paste via `zui_editor::clipboard`) by factoring them out of `Workspace` into a shared handler set, or
  add global `cx.on_action` forwarders that route to the workspace that owns the active database. Prefer a shared
  `register_code_actions(div, session)` helper used by both roots.
- Modals and dialogs (`Modal`, `ActiveDialog`) belong to `Workspace`; for a floating window, prompts should be shown in the
  main window (`activate_window` then open there) at first. Do not duplicate overlay machinery.
- `main.rs`: `on_window_closed` quit rule must count only the main window (closing the main window closes the floating
  ones; closing a floating one must not quit).
- `automation.rs`: add `window <n>` targeting, or have `action/key` go to `cx.active_window()`.
- `scripts/ui-capture.ps1`: capture is per window; do not use desktop capture.

Risks:
- Action handlers and dialogs as above (largest).
- Per-window state in async replies (window handles captured, see section 2).
- `Session` events fire once and each view reacts, so panels in the floating window update with no extra work; the risk is
  stale visibility flags (set the floating view visible on open, hidden on dock).
- Close and save flow: the floating window has no `should_close` prompt; the database-level prompt stays in the main
  window. Quit with floating windows open must not leave them orphaned (close them first).
- Multi-monitor DPI differences: gpui handles scale factor per window; fonts and `px` layout are per window, but cached
  measurements in views (`viewport_rows`, `text_w` in `ListingView`) must be recomputed on window change; they are painted
  each frame, so probably fine, but test dragging a window between monitors of different scale.
- Linux/Wayland has no window positioning; irrelevant on Windows but note it.
- A 50-line spike first (open a second window rendering a `ListingView` over the same `Session`) answers the open
  questions listed at the end of section 2.

Size: about 500-700 lines, dominated by action plumbing.

### Recommendation

Do (a) first and ship it, since it removes the "one view per kind" assumption that both other stages need. Do the `Origin` /
view-id refactor as its own commit. (b) is mostly layout once (a) has the model. (c) is mostly plumbing and best started
with the spike; consider shipping it after (b) only if the shared-handler refactor is acceptable, since it also benefits the
dialogs.
