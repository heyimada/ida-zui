# R1: the address the UI acts on is the address the focused view shows. Three scenarios, each from a fresh
# start: (a) Tab peek and back, then G to another function; (b) the same with a jump back to the start;
# (c) pinned tabs opened, navigated and closed. After each: CopyAddress (the isolated clipboard), the
# Pseudocode view, and a rename that must land on the visible function (read back by jumping to the new name).
param([string]$Work = "ui-focus")
$root = Split-Path $PSScriptRoot -Parent
$code = 0
function Run($goto, $log, $steps) {
    $global:LASTEXITCODE = 0
    & "$root\scripts\ui-capture.ps1" -Work "$Work-$log" -Goto $goto -Log $log -Steps $steps
    if ($LASTEXITCODE -ne 0) { $script:code = 1 }
}
$copy = "action:ida::CopyAddress", "wait:300"
function Rename($name, $addr) {
    @("key:n", "wait:800", "type:$name", "key:enter", "wait:3000",
      "key:g", "wait:300", "type:$name", "key:enter", "wait:2500") + $copy + @("clipassert:$addr")
}
# (a)
Run "mainCRTStartup" "focus_a" (@("wait:16000", "key:tab", "wait:3000", "key:tab", "wait:1500",
    "key:g", "wait:300", "type:FIND::Initialize", "key:enter", "wait:3000") + $copy + @("clipassert:1400018C0", "shot:focus_a_listing",
    "action:ida::ShowPseudocode", "wait:3000") + $copy + @("clipassert:1400018C0", "shot:focus_a_pseudo") + (Rename "izs_r1_a" "1400018C0"))
# (b)
Run "main" "focus_b" (@("wait:16000", "key:tab", "wait:3000", "key:tab", "wait:1500",
    "key:g", "wait:300", "type:mainCRTStartup", "key:enter", "wait:3000") + $copy + @("clipassert:140001300", "shot:focus_b_listing",
    "action:ida::ShowPseudocode", "wait:3000") + $copy + @("clipassert:140001300", "shot:focus_b_pseudo") + (Rename "izs_r1_b" "140001300"))
# (c)
Run "main" "focus_c" (@("wait:16000", "action:ida::NewGraphTab", "wait:2500",
    "key:g", "wait:300", "type:FIND::Initialize", "key:enter", "wait:3000",
    "action:ida::NewPseudoTab", "wait:3000", "key:ctrl-f4", "wait:1500") + $copy + @("clipassert:1400018C0", "shot:focus_c_graph",
    "key:ctrl-f4", "wait:1500") + $copy + @("clipassert:140002850", "shot:focus_c_main") + (Rename "izs_r1_c" "140002850"))
# (d) a pseudocode tab, "Open in new tab" from it, both closed again: back at the main view's own place
Run "main" "focus_d" (@("wait:16000", "action:ida::NewPseudoTab", "wait:3000", "action:ida::OpenInNewTab", "wait:2500", "shot:focus_d_tabs",
    "key:ctrl-f4", "wait:1500", "key:ctrl-f4", "wait:1500") + $copy + @("clipassert:140002850", "shot:focus_d_closed") + (Rename "izs_r1_d" "140002850"))
# (e) R2: a split on and off again leaves no tab behind
Run "main" "focus_e" @("wait:16000", "action:ida::ToggleSplit", "wait:2500", "shot:focus_e_split", "action:ida::ToggleSplit", "wait:1500", "action:ida::ToggleSplit", "wait:2000", "action:ida::ToggleSplit", "wait:1500", "shot:focus_e_off")
# (f) R4: an operand command on a line with no operand says so, and the submenu is greyed
Run "main" "focus_f" @("wait:16000", "key:h", "wait:600", "shot:focus_f_h", "action:ida::MenuEdit", "wait:600", "shot:focus_f_menu")
# (g) R5: Ctrl+C on a table row copies it as tab-separated columns
Run "main" "focus_g" @("wait:16000", "action:ida::ShowFunctions", "wait:800", "click:100,204", "wait:800", "key:ctrl-c", "wait:300", "clipassert:PROGRAM::Usage(void)`t_text`t1400016A0`t6`tthunk")
exit $code
