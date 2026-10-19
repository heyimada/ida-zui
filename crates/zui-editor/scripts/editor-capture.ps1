# Launches the zui-editor demo, captures ONLY its window with PrintWindow and
# saves it to artifacts/editor/<Name>.png. It never sends input and never takes
# the foreground: the window opens minimized, is restored without activation
# and pushed to the bottom of the z-order, then the process is killed.
# Example:
#   .\crates\zui-editor\scripts\editor-capture.ps1 -Name select -DemoArgs "--no-focus","--select","5:0-9:3"
param(
    [string]$Name = "demo",
    [string[]]$DemoArgs = @("--no-focus"),
    [int]$SettleMs = 2500,
    [string]$Exe = ""
)
$ErrorActionPreference = "Stop"
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$root = Split-Path $root -Parent
if (-not $Exe) { $Exe = Join-Path $root "target\debug\examples\demo.exe" }
$out = Join-Path $root "artifacts\editor"
New-Item -ItemType Directory -Force $out | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr p);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
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
$proc = Start-Process $Exe -ArgumentList $DemoArgs -PassThru -WindowStyle Minimized
try {
    $deadline = (Get-Date).AddSeconds(20)
    $h = [IntPtr]::Zero
    while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 200
        $h = [Win]::FindAppWindow([uint32]$proc.Id)
    }
    if ($h -eq [IntPtr]::Zero) { throw "demo window did not appear" }
    # SW_SHOWNOACTIVATE, then HWND_BOTTOM with SWP_NOMOVE|NOSIZE|NOACTIVATE.
    [Win]::ShowWindow($h, 4) | Out-Null
    [Win]::SetWindowPos($h, [IntPtr]1, 0, 0, 0, 0, 0x13) | Out-Null
    Start-Sleep -Milliseconds $SettleMs
    if ([Win]::IsIconic($h)) { throw "window is still minimized; PrintWindow would be black" }
    $r = New-Object Win+RECT
    [Win]::GetWindowRect($h, [ref]$r) | Out-Null
    $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $g.GetHdc()
    [Win]::PrintWindow($h, $hdc, 2) | Out-Null
    $g.ReleaseHdc($hdc)
    $g.Dispose()
    $file = Join-Path $out "$Name.png"
    $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    Write-Output "shot $file"
}
finally {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}
