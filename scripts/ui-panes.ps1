# H4 stage (b), two panes, per-tab history, tab keys and restoring the layout,
# end to end through the app's own automation channel. The view menu's split
# opens the pseudocode of the main view in a right pane; a jump moves both
# panes; a pinned tab goes back and forward on its own (Esc / Ctrl+Enter); Ctrl+PgDn
# cycles the tabs; a restart restores the panes and the tabs where they were.
# Addresses are read back with Copy current address through the in-process
# clipboard (`clipassert`); the system clipboard is never touched. Exit code 1
# if an assertion fails; screenshots are artifacts/ui/panes_*.png.
param([string]$Work = "panes")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:18000", "waitready", "action:ida::ToggleSplit", "wait:3000", "shot:panes_split",
    # a jump moves both panes (listing left, pseudocode right)
    "key:g", "wait:300", "type:SearchFiles", "key:enter", "wait:3000", "shot:panes_followed",
    # a pinned tab in the focused pane has its own history
    "action:ida::NewListingTab", "wait:2000",
    "key:g", "wait:300", "type:main", "key:enter", "wait:2500", "action:ida::CopyAddress", "wait:300", "clipassert:140002850",
    "key:escape", "wait:1500", "action:ida::CopyAddress", "wait:300", "clipassert:140002288",
    "key:ctrl-enter", "wait:1500", "action:ida::CopyAddress", "wait:300", "clipassert:140002850",
    "shot:panes_pinned",
    # next / previous tab, then the other pane
    "key:ctrl-pageup", "wait:1200", "shot:panes_prev_tab",
    "key:ctrl-pagedown", "wait:1200",
    "action:ida::MoveToOtherPane", "wait:2000", "shot:panes_moved",
    # the tab context menu
    "rightclick:695,120", "wait:600", "shot:panes_tabmenu", "key:escape", "wait:300",
    # drag a tab onto the other strip (it lands before the tab it is dropped on) ...
    "drag:695,120,930,120", "wait:1500", "shot:panes_dragged",
    # ... and reorder inside a strip (drop on the first tab: the dragged one goes before it)
    "drag:1090,120,880,120", "wait:1500", "shot:panes_reordered",
    # the divider moves with a drag (and the ratio is saved)
    "drag:822,400,700,400", "wait:1000", "shot:panes_divider"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log panes -Steps $steps
$code = $LASTEXITCODE
# Second start on the same settings (no --goto): the panes and the tabs come back
# where they were.
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Reuse -Log panes2 -Steps @("wait:9000", "waitready", "shot:panes_restored", "dblclick:700,400", "wait:800", "shot:panes_reset")
if ($LASTEXITCODE -ne 0) { $code = 1 }
exit $code
