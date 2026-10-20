# Drag-selecting past the bottom edge of the listing, the pseudocode and the hex view scrolls the view and
# keeps extending the selection: the copied text is much longer than what one screen shows.
# Screenshots: artifacts/ui/dragscroll_*.png.
param([string]$Work = "ui-dragscroll")
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:16000",
    # listing: more than twice the visible rows
    "action:ida::ShowListing", "wait:1500",
    "down:700,230", "heldmove:700,300", "heldmove:700,790", "wait:3500", "shot:dragscroll_listing", "up:700,790", "wait:300",
    "key:ctrl-c", "wait:300", "cliplines:45",
    # pseudocode
    "action:ida::ShowPseudocode", "wait:2500", "key:ctrl-home", "wait:500",
    "down:700,230", "heldmove:700,300", "heldmove:700,790", "wait:3500", "shot:dragscroll_pseudo", "up:700,790", "wait:300",
    "key:ctrl-c", "wait:300", "cliplines:45",
    # hex
    "action:ida::ShowHex", "wait:1500",
    "down:700,230", "heldmove:700,300", "heldmove:700,790", "wait:4000", "shot:dragscroll_hex", "up:700,790", "wait:300",
    "key:ctrl-c", "wait:300", "cliplen:2000"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::SearchFiles" -Log dragscroll -Steps $steps
exit $LASTEXITCODE
