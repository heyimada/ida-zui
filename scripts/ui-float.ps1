# A code view in a window of its own: pseudocode torn off from the tab strip, driven through the in-app
# automation channel (`win:1` aims at it). The window follows navigation like any other view, takes N
# (rename), X (xrefs) and the comment editor in itself, has the keymap and a context menu, and closing it
# returns the view to the strip. The first run leaves the window open and quits; the second run
# (same settings) must bring the window back where it was. Screenshots (window only):
# artifacts/ui/float_*.png.
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
    # X lists the references in that window, and the comment editor opens there too
    "key:x", "wait:2500", "shot:float_xrefs", "key:escape", "wait:400",
    "action:ida::Comment", "wait:1000", "shot:float_comment", "key:escape", "wait:400",
    # the keymap works there: copy the address of the cursor
    "action:ida::CopyAddress", "wait:300", "win:0", "clipassert:140002850",
    # a jump in the main window moves the floating view too
    "key:g", "wait:300", "type:SearchFiles", "key:enter", "wait:3000", "win:1", "shot:float_followed", "win:0",
    # the window stays open when the app quits
    "close", "wait:1500", "confirm:2", "waitexit:20000"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "main" -Log float -Steps $steps
$code = $LASTEXITCODE
$steps2 = @(
    "wait:12000", "shot:float_restored_main", "win:1", "shot:float_restored",
    # a second window, a listing: the structure chooser opens in it
    "win:0", "action:ida::NewListingTab", "wait:2000", "action:ida::MoveToNewWindow", "wait:3000",
    "win:2", "action:ida::OpStructOffset", "wait:2000", "shot:float_chooser", "win:1", "shot:float_chooser_other", "win:2", "key:escape", "wait:400",
    # a main-window command says so; the patch list opens in the window
    "action:ida::NextTab", "wait:500", "shot:float_unavailable",
    "action:ida::ShowPatches", "wait:1500", "shot:float_patches", "key:escape", "wait:400",
    # closing returns the views as tabs
    "action:ida::CloseTab", "wait:2000", "win:1", "action:ida::CloseTab", "wait:2500", "win:0", "shot:float_docked"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Reuse -Log float2 -Steps $steps2
if ($LASTEXITCODE -ne 0) { $code = 1 }
exit $code
