# H2 selection and copy, end to end through the app's own automation channel:
# selects lines or bytes in every code view (Shift+arrows, drag, Shift+click),
# copies with Ctrl+C / Copy with addresses / Copy as ..., and compares the
# in-process clipboard with the exact expected text (`clipassert`). The system
# clipboard is never read or written. Exit code 1 if any assertion fails; the
# log is artifacts/ui/selection.txt and the screenshots are artifacts/ui/sel_*.png.
param([string]$Work = "sel")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$vars3 = 'var_630 = qword ptr -630h\nvar_620 = byte ptr -620h\nvar_5F8 = byte ptr -5F8h'
$vars3addr = '1400018C0  var_630 = qword ptr -630h\n1400018C0  var_620 = byte ptr -620h\n1400018C0  var_5F8 = byte ptr -5F8h'
$steps = @(
    "wait:18000", "waitready",
    # --- listing: Shift+arrows, with addresses, Esc clears (and only that), drag, Shift+click
    "key:down", "key:down", "key:down", "key:shift-down", "key:shift-down", "wait:300", "shot:sel_listing",
    "key:ctrl-c", "wait:300", "clipassert:$vars3",
    "action:ida::CopyWithAddresses", "wait:300",
    "clipassert:.text:00000001400018C0  var_630 = qword ptr -630h\n.text:00000001400018C0  var_620 = byte ptr -620h\n.text:00000001400018C0  var_5F8 = byte ptr -5F8h",
    "key:escape", "wait:300", "key:ctrl-c", "wait:300", "clipassert:var_5F8 = byte ptr -5F8h",
    "click:666,427", "drag:666,427,666,467", "wait:300", "shot:sel_listing_drag", "key:ctrl-c", "wait:300", "clipassert:$vars3",
    "click:666,507", "wait:300", "shiftclick:666,567", "wait:300", "key:ctrl-c", "wait:300",
    "clipassert:var_5B0 = byte ptr -5B0h\nvar_57C = byte ptr -57Ch\nvar_570 = byte ptr -570h\nvar_550 = byte ptr -550h",
    # --- pseudocode
    "key:tab", "wait:1500", "key:down", "key:shift-down", "key:shift-down", "wait:300", "shot:sel_pseudo",
    "key:ctrl-c", "wait:300", "clipassert:{\n  bool v2; // bl\n  bool v4; // zf",
    "action:ida::CopyWithAddresses", "wait:300", "clipassert:1400018EC  {\n  bool v2; // bl\n  bool v4; // zf",
    # --- graph (a selection stays inside one block)
    "key:tab", "action:ida::ShowGraph", "wait:2000", "key:down", "key:down", "key:down", "key:shift-down", "key:shift-down", "wait:500", "shot:sel_graph",
    "key:ctrl-c", "wait:300", "clipassert:$vars3",
    "action:ida::CopyWithAddresses", "wait:300", "clipassert:$vars3addr",
    # --- hex: bytes, as hex / C array / Python bytes / a dump with addresses
    "action:ida::ShowHex", "wait:1500", "key:shift-down", "wait:500", "shot:sel_hex",
    "key:ctrl-c", "wait:300", "clipassert:48 8B C4 48 89 58 08 48 89 70 10 48 89 78 18 4C 89",
    "action:ida::CopyAsCArray", "wait:300",
    "clipassert:unsigned char data[] = {\n    0x48, 0x8B, 0xC4, 0x48, 0x89, 0x58, 0x08, 0x48, 0x89, 0x70, 0x10, 0x48, 0x89, 0x78, 0x18, 0x4C,\n    0x89,\n};",
    "action:ida::CopyAsPython", "wait:300",
    'clipassert:b"\x48\x8b\xc4\x48\x89\x58\x08\x48\x89\x70\x10\x48\x89\x78\x18\x4c\x89"',
    "action:ida::CopyWithAddresses", "wait:300",
    "clipassert:00000001400018C0  48 8B C4 48 89 58 08 48 89 70 10 48 89 78 18 4C\n00000001400018D0  89"
)
& "$root\scripts\ui-capture.ps1" -Work $Work -Goto "FIND::Initialize" -Log selection -Steps $steps
exit $LASTEXITCODE
