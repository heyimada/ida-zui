# Compiles scripts/debuggee/debuggee.c (a harmless test program for the
# debugger E2E) to testdata/debuggee/izs_debuggee.exe with MSVC.
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root "testdata\debuggee"
New-Item -ItemType Directory -Force $out | Out-Null
$src = Join-Path $PSScriptRoot "debuggee\debuggee.c"

# Use cl.exe from the environment, else find Visual Studio with vswhere.
$prefix = ""
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    $pf = if (${env:ProgramFiles(x86)}) { ${env:ProgramFiles(x86)} } else { "C:\Program Files (x86)" }
    $vswhere = Join-Path $pf "Microsoft Visual Studio\Installer\vswhere.exe"
    $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) { throw "Visual Studio C++ build tools not found" }
    $prefix = "`"$(Join-Path $vs 'VC\Auxiliary\Build\vcvars64.bat')`" >nul && "
}
Push-Location $out
try {
    # /Od keeps the functions as written, /INCREMENTAL:NO avoids jump thunks; /DYNAMICBASE makes the image base move.
    cmd /c "$prefix cl /nologo /Od /Zi /MT /Fe:izs_debuggee.exe `"$src`" /link /INCREMENTAL:NO /DYNAMICBASE /NXCOMPAT"
    if ($LASTEXITCODE -ne 0) { throw "cl failed" }
} finally { Pop-Location }
Write-Host (Join-Path $out "izs_debuggee.exe")
