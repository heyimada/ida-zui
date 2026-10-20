# H4 stage (a), code views as tabs, end to end through the app's own
# automation channel: Pseudocode of `main` in the main tab, a second
# Pseudocode tab opened at the cursor (pinned) and moved to FIND::SearchFiles,
# then back to the main tab, which still shows `main` (the cursor the status
# bar and breadcrumb show is read back with Copy current address and compared
# through the in-process clipboard; the system clipboard is never touched).
# Then the segmented control retargets the tab in front to the listing and
# Ctrl+click opens a call's target in a third tab. Exit code 1 if an assertion
# fails; screenshots are artifacts/ui/tabs_*.png.
param([string]$Work = "tabs")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:18000", "nocaption", "key:tab", "wait:2500",
    "action:ida::NewPseudoTab", "wait:2500", "shot:tabs_new",
    "key:g", "wait:300", "type:SearchFiles", "key:enter", "wait:3000", "shot:tabs_two",
    "action:ida::CopyAddress", "wait:300", "clipassert:140002288",
    "click:545,120", "wait:2000", "shot:tabs_main",
    "action:ida::CopyAddress", "wait:300", "clipassert:140002850",
    # the segmented control retargets the tab in front: the second tab becomes a listing
    "click:700,120", "wait:800", "click:668,81", "wait:2000", "shot:tabs_retarget"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "main" -Log tabs -Steps $steps
exit $LASTEXITCODE
