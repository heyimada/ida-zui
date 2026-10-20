# H1 context menus, end to end through the app's own automation channel:
# right-click on the listing, pseudocode, hex view, a side-list row and an
# inspector row, the Menu key at the caret, keyboard navigation
# (arrows wrap, Enter runs, Esc closes), and "right-click inside a selection
# keeps it". What entries copy is compared with the in-process clipboard
# (`clipassert`); the system clipboard is never touched. Exit code 1 if an
# assertion fails; the log is artifacts/ui/contextmenu.txt and the screenshots
# are artifacts/ui/ctx_*.png.
param([string]$Work = "ctx")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$kept = 'var_5F8 = byte ptr -5F8h\nvar_5D0 = byte ptr -5D0h\nvar_5B0 = byte ptr -5B0h'
$steps = @(
    "wait:18000",
    # --- listing: a right click opens the menu at the click
    "rightclick:666,427", "wait:600", "shot:ctx_listing", "key:escape", "wait:300",
    # --- the Menu key opens it at the caret with the first entry highlighted;
    #     Up wraps to the last entry, "Copy current address", and Enter runs it
    "click:666,427", "key:menu", "wait:500", "shot:ctx_listing_kbd", "key:up", "key:enter", "wait:500",
    "clipassert:1400018C0",
    # --- right-click inside a selection keeps it; outside it moves the caret
    "key:down", "key:down", "key:shift-down", "key:shift-down", "wait:300",
    "rightclick:666,467", "wait:400", "key:escape", "wait:300", "key:ctrl-c", "wait:300", "clipassert:$kept",
    "rightclick:666,527", "wait:400", "key:escape", "wait:300", "key:ctrl-c", "wait:300",
    "clipassert:var_57C = byte ptr -57Ch",
    # --- a side-list row: Down x4 reaches "Copy name"
    "rightclick:140,481", "wait:600", "shot:ctx_sidebar", "key:down", "key:down", "key:down", "key:down", "key:enter", "wait:500",
    "clipassert:MULTIPLE_PATH_ARGUMENT::GetLexemeThatFailed(void)",
    # --- an inspector row (opens flipped to the left near the window edge)
    "rightclick:1295,666", "wait:600", "shot:ctx_inspector", "key:escape", "wait:300",
    # --- pseudocode and hex
    "key:tab", "wait:1500", "rightclick:300,230", "wait:600", "shot:ctx_pseudo", "key:escape", "wait:300",
    "key:tab", "action:ida::ShowHex", "wait:1200", "rightclick:370,230", "wait:600", "shot:ctx_hex", "key:escape", "wait:300"
)
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log contextmenu -Steps $steps
exit $LASTEXITCODE
