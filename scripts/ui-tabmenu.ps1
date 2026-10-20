# Every item of the tab context menu: Copy title (asserted through the isolated clipboard), Move to other
# pane, Close to the right, Close others, Move to new window, Close (artifacts/ui/tabmenu_*.png).
param([string]$Work = "ui-tabmenu")
$root = Split-Path $PSScriptRoot -Parent
$steps = @(
    "wait:16000", "action:ida::NewListingTab", "wait:1200", "action:ida::NewGraphTab", "wait:1200", "action:ida::NewHexTab", "wait:1500",
    "shot:tabmenu_0",
    # Copy title (menu items sit at fixed offsets from the click)
    "rightclick:730,120", "wait:500", "click:765,321", "wait:400", "clipassert:Listing: FIND::SearchFiles",
    # Move to other pane: the graph tab goes right
    "rightclick:940,120", "wait:500", "click:975,261", "wait:1500", "shot:tabmenu_pane",
    # Close to the right of the listing tab: the hex tab goes
    "rightclick:730,120", "wait:500", "click:765,225", "wait:1200", "shot:tabmenu_right",
    # two tabs on the right again
    "action:ida::NewHexTab", "wait:1500", "shot:tabmenu_two",
    # Close others on the listing tab: the hex tab goes, the main view stays
    "rightclick:730,120", "wait:500", "click:765,199", "wait:1200", "shot:tabmenu_others",
    # Move to new window: the graph tab becomes a window
    "rightclick:930,120", "wait:500", "click:965,287", "wait:3000", "shot:tabmenu_main", "win:1", "shot:tabmenu_window", "win:0",
    # Close: the listing tab goes
    "rightclick:730,120", "wait:500", "click:765,173", "wait:1200", "shot:tabmenu_closed"
)
$global:LASTEXITCODE = 0
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::SearchFiles" -Log tabmenu -Steps $steps
exit $LASTEXITCODE
