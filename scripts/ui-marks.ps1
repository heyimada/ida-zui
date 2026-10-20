# M4 bookmarks, end to end through the app's own automation channel: mark a
# position (Alt+M, with a description), see the ribbon in the listing gutter,
# jump away, come back with Ctrl+M + Enter (the caret address is read back
# through the in-process clipboard with `clipassert`), then remove the mark
# with Del. The system clipboard is never touched. Exit code 1 if an assertion
# fails; the log is artifacts/ui/marks.txt, screenshots artifacts/ui/mark_*.png.
param([string]$Work = "mark")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:18000", "waitready",
    # land on `mov edx, 7530h` (1400019CF) and mark it
    "key:alt-i", "wait:500", "type:0x7530", "key:enter", "wait:1500",
    "key:alt-m", "wait:600", "shot:mark_prompt", "type:the immediate", "key:enter", "wait:1500", "shot:mark_gutter",
    # away to main, then back through the chooser
    "key:g", "wait:300", "type:main", "key:enter", "wait:1500", "action:ida::CopyAddress", "wait:300", "clipassert:140002850",
    "key:ctrl-m", "wait:800", "shot:mark_chooser", "key:enter", "wait:1500", "action:ida::CopyAddress", "wait:300", "clipassert:1400019CF",
    # Del removes it; the chooser has nothing left to offer
    "key:ctrl-m", "wait:800", "key:delete", "wait:1200", "shot:mark_removed",
    "key:ctrl-m", "wait:800", "shot:mark_none"
)
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log marks -Steps $steps
exit $LASTEXITCODE
