# M6 hex view, end to end through the app's own automation channel: the
# offset header and the group control, byte editing with F2 (digits overtype,
# Enter applies as a patch, Esc cancels), the patched bytes in their own
# colour, and the patch list. The bytes are read back by selecting them
# (Shift+Home) and copying through the in-process clipboard (`clipassert`);
# the system clipboard is never touched. Exit code 1 if an assertion fails; the
# log is artifacts/ui/hex.txt, screenshots artifacts/ui/hexv_*.png.
param([string]$Work = "hexv")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:18000", "waitready", "action:ida::ShowHex", "wait:1500", "shot:hexv_header",
    # the group control (the "2" button): bytes in pairs
    "click:1115,173", "wait:600", "shot:hexv_group2", "click:1157,173", "wait:600",
    # F2: a half-typed byte, then three bytes, applied with Enter
    "key:f2", "wait:400", "type:9", "wait:300", "shot:hexv_half", "type:090cc", "wait:400", "shot:hexv_typed", "key:enter", "wait:2500", "shot:hexv_patched",
    "key:shift-home", "key:ctrl-c", "wait:300", "clipassert:90 90 CC 48",
    # Esc cancels: the row still reads 90 90 CC 48 ... (no FF written)
    "key:escape", "key:f2", "type:ffff", "key:escape", "wait:500", "key:shift-end", "key:ctrl-c", "wait:300", "clipassert:90 90 CC 48 89 58 08 48 89 70 10 48 89 78 18 4C",
    "action:ida::ShowPatches", "wait:1000", "shot:hexv_patch_list"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log hex -Steps $steps
exit $LASTEXITCODE
