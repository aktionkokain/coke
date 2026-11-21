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

    # Build Coke Shot independently. Package it as an explicit optional
    # download instead of copying it into dist\mods with the base Coke mods.
    $cameraDir = Join-Path $root "mods\coke-camera"
    if (Test-Path $cameraDir) {
        Write-Host "== optional mod: Coke Shot =="
        Push-Location $cameraDir
        try {
            cargo build --release
            if ($LASTEXITCODE -ne 0) { throw "Coke Shot build failed" }
        } finally {
            Pop-Location
        }
        $optionalDir = Join-Path $root "dist\staging\Coke Shot"
        New-Item -ItemType Directory -Path $optionalDir -Force | Out-Null
        Copy-Item (Join-Path $cameraDir "target\i686-pc-windows-msvc\release\coke_camera.dll") $optionalDir -Force
        Copy-Item (Join-Path $cameraDir "coke-camera.ini") $optionalDir -Force
        Copy-Item (Join-Path $cameraDir "README.md") (Join-Path $optionalDir "README.txt") -Force
        Set-Content -LiteralPath (Join-Path $optionalDir "user_consent_coke_camera.txt") -Value "Coke Shot enabled by explicit installation." -Encoding ascii
        $zip = Join-Path $root "dist\Coke-Shot.zip"
        if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
        Compress-Archive -Path (Join-Path $optionalDir "*") -DestinationPath $zip
    }

    $modsOut = Join-Path $root "dist\mods"
    Write-Host "== mods: $modsOut =="
    Copy-CokeMods -Destination $modsOut

    Write-Host ""
    Write-Host "Proxy: target\i686-pc-windows-msvc\release\$($names.proxy_dll)"
    Write-Host "Mods : dist\mods\ (base mods; Coke Shot is separate in dist\Coke-Shot.zip)"
} finally {
    Pop-Location
}
