# Builds the proxy and the default mods for the 32-bit game process, then stages
# the mods into dist\mods\ together with their consent files.
#
# Native tools (cargo, rustup) write progress to stderr. Windows PowerShell 5.1
# turns that into a terminating error when $ErrorActionPreference is Stop, so use
# Continue and rely on the explicit $LASTEXITCODE checks below.
$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "names.ps1")
. (Join-Path $PSScriptRoot "mods.ps1")
$names = Get-CokeNames

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    rustup target add i686-pc-windows-msvc | Out-Null

    Write-Host "== proxy =="
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "proxy build failed" }

    foreach ($mod in Get-CokeMods) {
        $dir = Join-Path $root "mods\$($mod.Name)"
        if (-not (Test-Path $dir)) { continue }
        Write-Host "== mod: $($mod.Name) =="
        Push-Location $dir
        try {
            cargo build --release
            if ($LASTEXITCODE -ne 0) { throw "$($mod.Name) build failed" }
        } finally {
            Pop-Location
        }
    }

    $modsOut = Join-Path $root "dist\mods"
    Write-Host "== mods: $modsOut =="
    Copy-CokeMods -Destination $modsOut

    Write-Host ""
    Write-Host "Proxy: target\i686-pc-windows-msvc\release\$($names.proxy_dll)"
    Write-Host "Mods : dist\mods\ (DLL, .ini and user_consent_*.txt for each default mod)"
} finally {
    Pop-Location
}
