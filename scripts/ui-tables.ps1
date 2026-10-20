# H6 / L4 side-list tables, end to end through the app's own automation
# channel: header click sorts (numeric columns by value, names ignoring case,
# the arrow shows the direction), a column edge drags to resize, sort and
# widths persist across a restart, the Segments strip collapses and stays
# collapsed. The first row after a sort is read back with the context menu's
# "Copy name" and compared with the in-process clipboard (`clipassert`); the
# system clipboard is never touched. Exit code 1 if an assertion fails; the log
# is artifacts/ui/tables.txt, screenshots are artifacts/ui/tab_*.png.
param([string]$Work = "tab")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
# Down x4 from a fresh right-click menu is "Copy name" (Jump, Cross references, Rename, Copy name).
$copyName = @("rightclick:100,204", "wait:600", "key:down", "key:down", "key:down", "key:down", "key:enter", "wait:500")
$steps = @(
    "wait:18000", "shot:tab_functions",
    # Length ascending: by value, so 2 comes before 3, 6 and 0x10 (as text, "10" would lead)
    "click:392,180", "wait:700", "shot:tab_length_asc") + $copyName + @("clipassert:_guard_dispatch_icall_nop",
    # Start descending: the highest address first
    "click:290,180", "wait:500", "click:290,180", "wait:700", "shot:tab_start_desc") + $copyName + @("clipassert:_guard_dispatch_icall`$thunk`$10345483385596137414",
    # a column edge drags: Name wider, and the Start column moves right
    "drag:232,180,275,180", "wait:600", "shot:tab_resized",
    # Names: sorted by name ignoring case, `__C_specific_handler_0` before `__castguard...`
    "action:ida::ShowNames", "wait:1200", "click:150,180", "wait:700", "shot:tab_names_by_name") + $copyName + @("clipassert:__C_specific_handler_0",
    # the other lists
    "action:ida::ShowStrings", "wait:1200", "shot:tab_strings",
    "action:ida::ShowImports", "wait:1200", "shot:tab_imports",
    "action:ida::ShowExports", "wait:1200", "shot:tab_exports",
    # the Segments strip collapses
    "action:ida::ShowFunctions", "wait:1000", "click:50,724", "wait:600", "shot:tab_segments_collapsed")
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log tables -Steps $steps
$code = $LASTEXITCODE
# Second start on the same settings: the sort, the widths and the collapsed
# strip are still there.
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Reuse -Goto "FIND::Initialize" -Log tables2 -Steps @("wait:9000", "shot:tab_restored")
if ($LASTEXITCODE -ne 0) { $code = 1 }
exit $code
