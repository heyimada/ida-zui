# UI capture driver. Launches ida-zui on a fresh copy of a sample binary and
# drives it through the app's own automation channel (IDA_ZUI_AUTOMATION):
# commands are appended to a file that the app polls and executes inside its
# window. Nothing is ever sent to the OS input queue, so no other window can
# receive keystrokes or clicks; the app window is started without focus and
# pushed to the bottom of the z-order. Screenshots use PrintWindow on the app
# window only.
#
# Steps (-Steps "a","b",...):
#   wait:<ms>          sleep
#   key:<keystroke>    gpui keystroke through the keybindings: ctrl-shift-p, n, enter, escape, alt, f10
#   type:<text>        typed into the focused input
#   action:<ns::Name>  dispatch an action by name, e.g. ida::Rename
#   move:<x>,<y>       mouse move, logical window pixels (content area)
#   click:<x>,<y>      move + left click, same coordinates
#   dblclick:<x>,<y>   click twice, the second with click count 2
#   shiftclick:<x>,<y> click with Shift held; rightclick:<x>,<y> a right click
#   down:<x>,<y> / heldmove:<x>,<y> / up:<x>,<y>  the left button held across steps (drag-select past an edge)
#   drag:<x1>,<y1>,<x2>,<y2>  press at the first point, move with the button held, release at the second
#   confirm:<n>        answer button n (1-based) of an in-app question
#   drop:<path>[|<path>]  simulate files dropped on the window (in-process, via gpui's file-drop events)
#   clipboard:<text>   seed the app's in-process clipboard (the system clipboard is never touched)
#   clipdump           print the in-process clipboard (and log it; newlines show as \n)
#   clipassert:<text>  like clipdump, then compare with <text> (write newlines as \n); a mismatch fails the script
#   marker:<name>      wait until the app has executed everything queued so far
#   shot:<name>        save the window as artifacts/ui/<name>.png
#   close              post WM_CLOSE (no focus change), waitexit:<ms>, files:<glob>
#   procs              list the ida-host.exe processes that are children of the app (their PIDs are remembered)
#   hostsgone[:<ms>]   wait up to <ms> (default 20000) for the hosts remembered by `procs` to exit while the app is
#                      still running; FAILS the script (exit 1) if one is left, or if the app itself already
#                      exited (a host also dies with its parent, so that check would prove nothing)
# Coordinates are logical pixels; the PNG is physical pixels (scale in artifacts/ui/scale.txt).
# Switches: -NoFile (start on the welcome screen), -Reuse (keep work dir, settings and .i64),
# -Settings '<json>' (seed settings.json), -Release, -Keep, -Log NAME (step log to artifacts/ui/NAME.txt),
# -Samples a.exe,b.dll (several databases: each is copied and passed on the command line; -Sample is ignored),
# -Exe PATH (run another build of ida-zui.exe, e.g. a saved older one; ida-host.exe must sit next to it),
# -Work NAME (use testdata\NAME instead of testdata\ui, so parallel runs do not wipe each other).
# Example:
#   .\scripts\ui-capture.ps1 -Goto "FIND::Initialize" -Steps "wait:16000","key:alt","key:f","shot:menu"
param(
    [string]$Sample = "C:\Windows\System32\find.exe",
    [string[]]$Samples = @(),
    [string]$Work = "ui",
    [string]$Goto = "",
    [string[]]$Steps = @("wait:15000", "shot:main"),
    [string]$Settings = "",
    [switch]$NoFile,
    [switch]$Keep,
    [switch]$Reuse,
    [switch]$Release,
    [string]$Log = "",
    [string]$Exe = ""
)
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root "artifacts\ui"
New-Item -ItemType Directory -Force $out | Out-Null
$work = Join-Path $root ("testdata\" + $Work)
if (-not $Reuse) { Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue }
New-Item -ItemType Directory -Force $work | Out-Null
if ($Samples.Count -eq 0) { $Samples = @($Sample) }
$copies = @()
foreach ($smp in $Samples) {
    $c = Join-Path $work (Split-Path $smp -Leaf)
    if (-not $NoFile -and -not (Test-Path $c)) { Copy-Item $smp $c }
    $copies += $c
}
$hostPids = @()
$failed = $false
# Settings live in the work dir so captures never touch the user's own.
$config = Join-Path $work "config"
New-Item -ItemType Directory -Force $config | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
if ($Settings) { [System.IO.File]::WriteAllText((Join-Path $config "settings.json"), $Settings, $utf8) }
$channel = Join-Path $work "automation.txt"
[System.IO.File]::WriteAllText($channel, "", $utf8)
Remove-Item "$channel.done" -ErrorAction SilentlyContinue
$logLines = New-Object System.Collections.Generic.List[string]
function Note($m) { Write-Output $m; $logLines.Add($m) }
function Send($line) { [System.IO.File]::AppendAllText($channel, "$line`n", $utf8) }

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
  // Behind every other window, without activating: the app is driven in-process, it never needs the foreground.
  public static void SendToBottom(IntPtr h) {
    ShowWindow(h, 4);                                   // SW_SHOWNOACTIVATE
    SetWindowPos(h, (IntPtr)1, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);  // HWND_BOTTOM, NOSIZE|NOMOVE|NOACTIVATE
  }
  public static bool HasConsole(uint pid) {
    bool found = false;
    EnumWindows((h, p) => {
      uint wp; GetWindowThreadProcessId(h, out wp);
      var sb = new System.Text.StringBuilder(256); GetClassName(h, sb, 256);
      if (wp == pid && sb.ToString() == "ConsoleWindowClass") found = true;
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static IntPtr FindAppWindow(uint pid) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, p) => {
      uint wp; GetWindowThreadProcessId(h, out wp);
      if (wp != pid || !IsWindowVisible(h)) return true;
      var sb = new System.Text.StringBuilder(256); GetClassName(h, sb, 256);
      if (sb.ToString() == "ConsoleWindowClass") return true;
      found = h; return false;
    }, IntPtr.Zero);
    return found;
  }
}
"@

[Win]::SetProcessDPIAware() | Out-Null
$exe = if ($Exe) { $Exe } else { Join-Path $root ("target\" + $(if ($Release) { "release" } else { "debug" }) + "\ida-zui.exe") }
$argList = @()
if (-not $NoFile) { foreach ($c in $copies) { $argList += "`"$c`"" } }
if ($Goto) { $argList += @("--goto", "`"$Goto`"") }
$env:IDA_ZUI_AUTOMATION = $channel
$env:IDA_ZUI_CONFIG_DIR = $config
try {
    # Minimized at start: the window is created without being activated.
    $proc = Start-Process $exe -ArgumentList $argList -PassThru -WindowStyle Minimized
}
finally {
    Remove-Item Env:\IDA_ZUI_AUTOMATION, Env:\IDA_ZUI_CONFIG_DIR -ErrorAction SilentlyContinue
}
try {
    $deadline = (Get-Date).AddSeconds(20)
    $h = [IntPtr]::Zero
    while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 200
        $h = [Win]::FindAppWindow([uint32]$proc.Id)
    }
    if ($h -eq 0) { throw "ida-zui window did not appear" }
    [Win]::SendToBottom($h)
    Note "console window present: $([Win]::HasConsole([uint32]$proc.Id))"
    $scale = [Win]::GetDpiForWindow($h) / 96.0
    Set-Content (Join-Path $out "scale.txt") "$scale"
    foreach ($step in $Steps) {
        $kind, $arg = $step -split ":", 2
        if ($proc.HasExited -and $kind -notin "hostsgone", "files", "procs") { Note "app exited before step '$step'"; break }
        switch ($kind) {
            "wait" { Start-Sleep -Milliseconds ([int]$arg) }
            "key" { Send "key $arg" }
            "type" { Send "type $arg" }
            "action" { Send "action $arg" }
            "confirm" { Send "confirm $arg" }
            "move" { Send ("move " + ($arg -replace ",", " ")) }
            "click" { Send ("click " + ($arg -replace ",", " ")) }
            "dblclick" { Send ("dblclick " + ($arg -replace ",", " ")) }
            "shiftclick" { Send ("shiftclick " + ($arg -replace ",", " ")) }
            "rightclick" { Send ("rightclick " + ($arg -replace ",", " ")) }
            "drag" { Send ("drag " + ($arg -replace ",", " ")) }
            "down" { Send ("down " + ($arg -replace ",", " ")) }
            "heldmove" { Send ("heldmove " + ($arg -replace ",", " ")) }
            "up" { Send ("up " + ($arg -replace ",", " ")) }
            "clipassert" {
                $syncName = "clipassert-" + [guid]::NewGuid().ToString("N")
                Send "clipdump"
                Send "marker $syncName"
                $done = Join-Path $work "automation.txt.done"
                $until = (Get-Date).AddSeconds(30)
                while ((Get-Date) -lt $until -and -not ((Test-Path $done) -and ((Get-Content $done) -contains $syncName))) { Start-Sleep -Milliseconds 100 }
                $line = Get-Content $done | Where-Object { $_ -like "clipboard:*" } | Select-Object -Last 1
                $got = if ($line) { $line.Substring("clipboard: ".Length) } else { "<none>" }
                if ($got -ceq $arg) { Note "PASS clipassert: $got" }
                else { Note "FAIL clipassert: expected [$arg] got [$got]"; $failed = $true }
            }
            "clipboard" { Send "clipboard $arg" }
            "drop" { Send "drop $arg" }
            "clipdump" {
                Send "clipdump"
                Send "marker clipdump-sync"
                $done = Join-Path $work "automation.txt.done"
                $until = (Get-Date).AddSeconds(30)
                while ((Get-Date) -lt $until -and -not ((Test-Path $done) -and ((Get-Content $done) -contains "clipdump-sync"))) { Start-Sleep -Milliseconds 100 }
                Get-Content $done | Where-Object { $_ -like "clipboard:*" } | Select-Object -Last 1 | ForEach-Object { Note $_ }
            }
            "marker" {
                Send "marker $arg"
                $done = Join-Path $work "automation.txt.done"
                $until = (Get-Date).AddSeconds(60)
                while ((Get-Date) -lt $until -and -not ((Test-Path $done) -and ((Get-Content $done) -contains $arg))) {
                    Start-Sleep -Milliseconds 100
                }
                Note "marker $arg reached: $((Test-Path $done) -and ((Get-Content $done) -contains $arg))"
            }
            { $_ -in "shot", "screen" } {
                # PrintWindow renders the app's own content, wherever it sits in the z-order.
                [Win]::SendToBottom($h)
                Start-Sleep -Milliseconds 250
                $r = New-Object Win+RECT
                [Win]::GetClientRect($h, [ref]$r) | Out-Null
                $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
                $g = [System.Drawing.Graphics]::FromImage($bmp)
                $hdc = $g.GetHdc()
                [Win]::PrintWindow($h, $hdc, 3) | Out-Null   # PW_CLIENTONLY | PW_RENDERFULLCONTENT
                $g.ReleaseHdc($hdc)
                $g.Dispose()
                $file = Join-Path $out "$arg.png"
                $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
                $bmp.Dispose()
                Note "shot $file"
            }
            "procs" {
                $kids = @(Get-CimInstance Win32_Process -Filter "Name='ida-host.exe' AND ParentProcessId=$($proc.Id)" | ForEach-Object { $_.ProcessId })
                $hostPids = @($hostPids + $kids | Select-Object -Unique)
                Note "ida-host children of pid $($proc.Id): $($kids.Count) [$($kids -join ', ')]"
            }
            "hostsgone" {
                if ($proc.HasExited) {
                    Note "FAIL hostsgone: the app has already exited, so hosts would exit with it; run this step while the app is alive"
                    $failed = $true
                    break
                }
                $limit = if ($arg) { [int]$arg } else { 20000 }
                $until = (Get-Date).AddMilliseconds($limit)
                do {
                    $alive = @($hostPids | Where-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
                    if ($alive.Count -eq 0) { break }
                    Start-Sleep -Milliseconds 200
                } while ((Get-Date) -lt $until)
                Note "remembered hosts [$($hostPids -join ', ')]: still running = $($alive.Count) [$($alive -join ', ')], app alive = $(-not $proc.HasExited)"
                if ($hostPids.Count -eq 0) { Note "FAIL hostsgone: no host was remembered (run procs first)"; $failed = $true }
                elseif ($alive.Count -gt 0) { Note "FAIL hostsgone: host processes outlived their tabs"; $failed = $true }
            }
            "close" {
                [Win]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
                Note "posted WM_CLOSE"
            }
            "waitexit" {
                $exited = $proc.WaitForExit([int]$arg)
                Note "app exited within $arg ms: $exited"
            }
            "files" {
                $found = Get-ChildItem $work -Filter $arg -ErrorAction SilentlyContinue | ForEach-Object { "$($_.Name) ($($_.Length) bytes)" }
                Note "files $arg -> $(if ($found) { $found -join ', ' } else { '(none)' })"
            }
            default { Note "unknown step '$step'" }
        }
    }
}
finally {
    if ($Log) { $logLines | Set-Content (Join-Path $out "$Log.txt") }
    if (-not $Keep) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
}
if ($failed) { exit 1 }
