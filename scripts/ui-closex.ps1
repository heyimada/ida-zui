# The title strip's own close button must go through the same unsaved-changes
# prompt as the OS close: rename (dirty), click X, answer Save, and the packed
# .i64 must exist with no unpacked .id0 left behind. Exit code 1 otherwise.
param([string]$Work = "closex", [string]$Exe = "")
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:16000", "waitready", "key:n", "wait:400", "type:closex_dirty", "key:enter", "wait:1500",
    "click:1455,20", "wait:1500", "shot:closex_prompt", "confirm:1", "waitexit:30000"
)
$opts = @{ Work = $Work; Goto = "main"; Log = "closex"; Steps = $steps }
if ($Exe) { $opts.Exe = $Exe }
& "$root\scripts\ui-capture.ps1" @opts
$dir = Join-Path $root "testdata\$Work"
$i64 = Test-Path (Join-Path $dir "find.exe.i64")
$id0 = Test-Path (Join-Path $dir "find.exe.id0")
if ($i64 -and -not $id0) { Write-Output "PASS closex: saved find.exe.i64, no unpacked files left"; exit 0 }
Write-Output "FAIL closex: i64=$i64 id0=$id0"; exit 1
