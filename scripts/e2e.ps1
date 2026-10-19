# Builds everything, copies a fresh sample binary into testdata/ (IDA writes
# databases next to its input) and runs the protocol E2E check, then the
# debugger E2E against a small program compiled from scripts/debuggee.
# Artifacts: artifacts/e2e/<binary>.txt (diffable dump) and .log (IDA output),
# artifacts/e2e/debug.txt (debugger sessions).
param([string]$Sample = "C:\Windows\System32\find.exe")
$ErrorActionPreference = "Stop"
# Belt and braces: the host refuses any Lumina push in this environment.
$env:IZS_BLOCK_LUMINA_PUSH = "1"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
cargo build --workspace --bins --examples
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$work = Join-Path $root "testdata\e2e"
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $work | Out-Null
$copy = Join-Path $work (Split-Path $Sample -Leaf)
Copy-Item $Sample $copy
& (Join-Path $root "target\debug\examples\e2e.exe") $copy (Join-Path $root "artifacts\e2e")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# Debugger: only a program we compile ourselves is ever run.
& (Join-Path $PSScriptRoot "build-debuggee.ps1")
$debuggee = Join-Path $root "testdata\debuggee\izs_debuggee.exe"
try {
    & (Join-Path $root "target\debug\examples\debug_e2e.exe") $debuggee (Join-Path $root "artifacts\e2e")
    $code = $LASTEXITCODE
} finally {
    # A failed assertion must not leave a debuggee behind (a host exits with its pipe).
    Get-Process izs_debuggee -ErrorAction SilentlyContinue | Stop-Process -Force
}
exit $code
