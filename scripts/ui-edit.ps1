# H3 editing commands, end to end through the app's own automation channel,
# on a fresh copy of find.exe: operand representations (keys and the menu),
# the array dialog, string types, change bytes, assemble, revert, the patched
# bytes list, apply patches to a copy of the input file, and Edit function.
# After each edit the text of the line is copied (Ctrl+C) and compared exactly
# with the in-process clipboard (`clipassert`); the system clipboard is never
# touched. Exit code 1 if an assertion fails; the log is
# artifacts/ui/edit.txt and the screenshots are artifacts/ui/edit_*.png.
param([string]$Work = "edit")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
function Line($expect) { @("key:ctrl-c", "wait:300", "clipassert:$expect") }
$steps = @(
    "wait:18000",
    "files:find.exe",
    # --- operands on `mov edx, 7530h` (found by its immediate; the caret goes on 7530h)
    "key:alt-i", "wait:500", "type:0x7530", "key:enter", "wait:1500", "click:700,368", "wait:800",
    "key:h", "wait:1500", "key:h", "wait:1500") +
    (Line "        mov     edx, 30000") + @("key:b", "wait:1500") +
    (Line "        mov     edx, 111010100110000b") + @("key:r", "wait:1500") +
    (Line "        mov     edx, 'u0'") + @("action:ida::OpClear", "wait:1500") +
    (Line "        mov     edx, 7530h") + @("action:ida::OpInvert", "wait:1500") +
    (Line "        mov     edx, not 0FFFF8ACFh") + @("action:ida::OpClear", "wait:1500", "action:ida::OpNegate", "wait:1500") +
    (Line "        mov     edx, -0FFFF8AD0h") + @(
    "action:ida::OpClear", "wait:1500", "key:b", "wait:1500",
    # the Edit > Operand types submenu shows what the operand is now
    "key:alt-e", "wait:400", "key:o", "wait:500", "shot:edit_operand_menu", "key:escape", "key:escape", "wait:300",
    "action:ida::OpClear", "wait:1500",
    # --- the structure chooser (T) and a right-click menu with the Operand submenu
    "key:t", "wait:1000", "shot:edit_struct_chooser", "key:escape", "wait:300",
    # --- assemble over it: `nop`, padded with nops to the old size
    "action:ida::PatchAssemble", "wait:1000", "shot:edit_assemble", "type:nop", "key:enter", "wait:2500") +
    (Line "        nop") + @("shot:edit_assembled",
    # --- an array at the first data item
    "key:g", "wait:300", "type:140006000", "key:enter", "wait:1500", "key:down", "key:down", "key:down", "wait:500",
    "action:ida::DefineArray", "wait:1500", "key:ctrl-a", "type:4", "wait:600", "shot:edit_array", "key:enter", "wait:2500", "key:down", "key:down", "key:down", "wait:500") +
    (Line "__defaultmatherr dd 1, 3 dup(0) ; DATA XREF: pre_c_init+B6$([char]0x2191)r") + @("shot:edit_array_done",
    # --- change bytes inside the padding after it, then make them a UTF-16 string
    "key:g", "wait:300", "type:140006020", "key:enter", "wait:1500",
    "action:ida::PatchChangeBytes", "wait:1200", "shot:edit_bytes", "key:ctrl-a", "type:48 00 69 00 00 00", "key:enter", "wait:2500",
    "action:ida::SetStringType", "wait:800", "shot:edit_string_type", "type:UTF-16", "wait:400", "key:enter", "wait:2500") +
    (Line "        text `"UTF-16LE`", 'Hi',0") + @("shot:edit_string",
    "action:ida::ShowPatches", "wait:1200", "shot:edit_patched_bytes", "key:escape", "wait:300",
    # --- a copy of the input with the patches; the input itself keeps its size
    "action:ida::PatchApply", "wait:1000", "shot:edit_apply", "key:enter", "wait:2500", "shot:edit_applied",
    "files:find.exe*",
    # --- revert the string's bytes; the five assembled bytes are still listed
    "action:ida::PatchRevert", "wait:2500") +
    (Line "        text `"UTF-16LE`", 0,0,0") + @("action:ida::ShowPatches", "wait:1200", "shot:edit_no_patches", "key:escape", "wait:300",
    # --- Edit function: the dialog, then a rename through it
    "key:g", "wait:300", "type:FIND::Initialize", "key:enter", "wait:2000", "action:ida::EditFunctionProps", "wait:1500", "shot:edit_function",
    "key:ctrl-a", "type:InitializeRenamed", "key:enter", "wait:2500", "shot:edit_function_done")
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log edit -Steps $steps
$code = $LASTEXITCODE
# The patched copy differs from the untouched input in exactly the patched
# bytes: `nop` x5 over `mov edx, 7530h`, and "Hi" in the data padding.
$in = [System.IO.File]::ReadAllBytes((Join-Path $root "testdata\$Work\find.exe"))
$out = [System.IO.File]::ReadAllBytes((Join-Path $root "testdata\$Work\find.exe.patched"))
$diff = 0
for ($i = 0; $i -lt [Math]::Min($in.Length, $out.Length); $i++) { if ($in[$i] -ne $out[$i]) { $diff++ } }
$line = "patched copy: $($out.Length) bytes, input $($in.Length) bytes, $diff bytes differ (expected 7)"
Write-Output $line
Add-Content (Join-Path $root "artifacts\ui\edit.txt") $line
if ($in.Length -ne $out.Length -or $diff -ne 7) { Write-Output "FAIL patched copy"; $code = 1 }
exit $code
