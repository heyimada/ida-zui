# Builds everything, copies a fresh sample binary into testdata/ (IDA writes
# databases next to its input) and runs the protocol E2E check.
# Artifacts: artifacts/e2e/<binary>.txt (diffable dump) and .log (IDA output).
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
exit $LASTEXITCODE
