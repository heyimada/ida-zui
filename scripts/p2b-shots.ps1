# Captures the six design screens (S1 main, S2 graph, S3 menus, S4 settings,
# S5 welcome + open options, S6 palette + rename) in Dark and Light through the
# app's automation channel. Output: artifacts/ui/p2b_<screen>_<mode>.png
# Usage: .\scripts\p2b-shots.ps1 [-Modes Dark,Light] [-Screens main,graph,...]
param(
    [string[]]$Modes = @("Dark", "Light"),
    [string[]]$Screens = @("main", "graph", "menus", "settings", "welcome", "palette")
)
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$recent = '[{"path":"C:\\Windows\\System32\\find.exe","opened":NOW},{"path":"C:\\Windows\\System32\\shell32.dll","opened":NOW_1},{"path":"C:\\Windows\\System32\\notepad.exe","opened":NOW_2}]'
$now = [int][double]::Parse((Get-Date -UFormat %s))
$recent = $recent.Replace("NOW_1", $now - 86400).Replace("NOW_2", $now - 259200).Replace("NOW", $now - 120)

foreach ($mode in $Modes) {
    $m = $mode.ToLower()
    $split = '"layout":{"split":true}'
    foreach ($screen in $Screens) {
        switch ($screen) {
            "main" {
                & "$here\ui-capture.ps1" -Goto "0x1400022d9" -Settings "{`"theme`":`"$mode`",$split}" -Steps "wait:18000", "shot:p2b_main_$m" | Out-Null
            }
            "graph" {
                & "$here\ui-capture.ps1" -Goto "0x1400022d9" -Settings "{`"theme`":`"$mode`",$split}" -Steps "wait:18000", "action:ida::ShowGraph", "wait:2500", "shot:p2b_graph_$m" | Out-Null
            }
            "menus" {
                & "$here\ui-capture.ps1" -Goto "0x1400022d9" -Settings "{`"theme`":`"$mode`",$split}" -Steps "wait:18000", "key:alt-f", "wait:600", "shot:p2b_menus_file_$m", "key:escape", "key:alt-o", "key:right", "wait:600", "shot:p2b_menus_theme_$m" | Out-Null
            }
            "settings" {
                & "$here\ui-capture.ps1" -Goto "0x1400022d9" -Settings "{`"theme`":`"$mode`",$split}" -Steps "wait:18000", "action:ida::OpenKeymap", "wait:800", "shot:p2b_settings_$m" | Out-Null
            }
            "welcome" {
                & "$here\ui-capture.ps1" -NoFile -Settings "{`"theme`":`"$mode`",`"recent`":$recent}" -Steps "wait:2500", "shot:p2b_welcome_$m", "action:ida::OpenWithOptions", "wait:600", "type:C:\Windows\System32\find.exe", "click:738,426", "wait:600", "shot:p2b_open_options_$m" | Out-Null
            }
            "palette" {
                & "$here\ui-capture.ps1" -Goto "0x1400022d9" -Settings "{`"theme`":`"$mode`",$split}" -Steps "wait:18000", "key:ctrl-p", "wait:800", "type:search", "wait:600", "shot:p2b_palette_$m", "key:escape", "wait:500", "key:n", "wait:800", "shot:p2b_rename_$m" | Out-Null
            }
        }
        Write-Output "done $screen $mode"
    }
}
