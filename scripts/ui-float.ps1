# A code view in a window of its own: pseudocode torn off from the tab strip, driven through the in-app
# automation channel (`win:1` aims at it). The window follows navigation like any other view, takes N
# (rename) with its own prompt, and closing it returns the view to the strip. Screenshots (window only):
# artifacts/ui/float_main.png, float_window.png, float_prompt.png, float_renamed.png, float_menu.png, float_followed.png, float_docked.png.
param([string]$Work = "ui-float")
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:18000",
    # the pseudocode of main becomes a window
    "action:ida::NewPseudoTab", "wait:2500", "action:ida::MoveToNewWindow", "wait:3500",
    "shot:float_main", "win:1", "shot:float_window",
    # typing works there: rename the function with N
    "key:n", "wait:800", "type:izs_float_name", "wait:300", "shot:float_prompt", "key:enter", "wait:3500", "shot:float_renamed",
    # a context menu opens in it
    "action:ida::ShowContextMenu", "wait:600", "shot:float_menu", "key:escape", "wait:300",
    # the keymap works there: copy the address of the cursor
    "action:ida::CopyAddress", "wait:300", "win:0", "clipassert:140002850",
    # a jump in the main window moves the floating view too
    "key:g", "wait:300", "type:SearchFiles", "key:enter", "wait:3000", "win:1", "shot:float_followed", "win:0",
    # closing returns the view as a tab
    "win:1", "action:ida::CloseTab", "wait:2500", "win:0", "shot:float_docked"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "main" -Log float -Steps $steps
exit $LASTEXITCODE
