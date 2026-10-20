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
    "wait:18000", "action:ida::ToggleSplit", "wait:3000", "shot:panes_split",
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
    "action:ida::MoveToOtherPane", "wait:2000", "shot:panes_moved"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log panes -Steps $steps
$code = $LASTEXITCODE
# Second start on the same settings (no --goto): the panes and the tabs come back
# where they were.
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Reuse -Log panes2 -Steps @("wait:9000", "shot:panes_restored")
if ($LASTEXITCODE -ne 0) { $code = 1 }
exit $code
