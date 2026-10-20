# L2 Hide casts (`\`), end to end through the app's own automation channel:
# the pseudocode of `main` with and without casts (screenshots), then a click
# on the `FIND::Terminate((FIND *)v6)` call with the casts hidden and a rename:
# the prompt names the call's target (140002830) and the rename lands on that
# line. The new line is read back with Ctrl+C and compared with the in-process
# clipboard (`clipassert`); the system clipboard is never touched. Exit code 1
# if an assertion fails; screenshots are artifacts/ui/cast_*.png.
param([string]$Work = "cast")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:18000", "key:tab", "wait:2500", "shot:cast_on",
    "key:\", "wait:2500", "shot:cast_off",
    "click:520,634", "wait:500", "key:n", "wait:700", "shot:cast_prompt",
    "key:ctrl-a", "type:TerminateRenamed", "key:enter", "wait:3500", "shot:cast_renamed",
    "key:ctrl-c", "wait:300", "clipassert:  TerminateRenamed(v6);",
    # casts come back with the same key
    "key:\", "wait:2500", "key:ctrl-c", "wait:300", "clipassert:  TerminateRenamed((FIND *)v6);"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "main" -Log casts -Steps $steps
exit $LASTEXITCODE
