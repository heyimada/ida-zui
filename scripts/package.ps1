# Builds the release binaries and zips them into dist/ida-zui-<version>-windows-x64.zip:
# ida-zui.exe, ida-host.exe, LICENSE, THIRD_PARTY.md and a short README.txt.
# The package needs IDA Pro with idalib installed on the target machine
# and nothing else (the SDK is only used at build time).
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
cargo build --workspace --bins --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
$name = "ida-zui-$version-windows-x64"
$stage = Join-Path $root "dist\$name"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($f in "ida-zui.exe", "ida-host.exe") { Copy-Item (Join-Path $root "target\release\$f") $stage }
foreach ($f in "LICENSE", "THIRD_PARTY.md") { Copy-Item (Join-Path $root $f) $stage }
@"
ida-zui $version - a modern front end for IDA Pro (Windows x64)

Requirements
  * IDA Pro with idalib and a valid licence, installed on this machine.
    ida-zui finds the newest "IDA *" folder under Program Files that contains
    idalib.dll. If yours is elsewhere, start ida-zui and use "Locate IDA..."
    (or set the IDADIR environment variable, which takes precedence).
  * The Hex-Rays decompiler is needed for pseudocode only.

Run
  Keep ida-zui.exe and ida-host.exe in the same folder, then start ida-zui.exe,
  drop a binary on the window or press Ctrl+O. Command line:
      ida-zui.exe [FILE...] [--goto NAME]
  Opening a binary creates <binary>.i64 next to it, as IDA does.

Help
  Ctrl+Shift+P opens the command palette (every command is searchable);
  Help > Keyboard shortcuts lists the keys. Settings: %APPDATA%\ida-zui\settings.json.
  Source and documentation: https://github.com/heyimada/ida-zui

ida-zui is not affiliated with Hex-Rays. It contains no IDA files.
"@ | Set-Content -Encoding UTF8 (Join-Path $stage "README.txt")
$zip = Join-Path $root "dist\$name.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip
Write-Output "wrote $zip"
