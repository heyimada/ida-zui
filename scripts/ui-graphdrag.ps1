# Drag-selecting in a graph block past the bottom edge of the canvas pans the graph and keeps extending
# the selection (artifacts/ui/graphdrag_start.png, graphdrag_held.png, graphdrag_end.png).
param([string]$Work = "ui-graphdrag")
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:16000", "action:ida::ShowGraph", "wait:3000", "shot:graphdrag_start",
    # press inside the first block, then hold the pointer below the canvas
    "down:600,440", "heldmove:600,500", "heldmove:600,780", "wait:2500", "shot:graphdrag_held",
    "up:600,780", "wait:500", "shot:graphdrag_end"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::SearchFiles" -Log graphdrag -Steps $steps
exit $LASTEXITCODE
