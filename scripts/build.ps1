# Native tools (cargo, rustup) write progress to stderr. Windows PowerShell 5.1
# turns that into a terminating error when $ErrorActionPreference is Stop, so use
# Continue and rely on the explicit $LASTEXITCODE checks below.
$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "mods.ps1")

$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    rustup target add i686-pc-windows-msvc | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Could not install the x86 Rust target" }

    Write-Host "== proxy =="
    cargo build --locked --release
    if ($LASTEXITCODE -ne 0) { throw "proxy build failed" }

    foreach ($mod in Get-CokeMods) {
        $dir = Join-Path $root "mods\$($mod.Name)"
        Write-Host "== mod: $($mod.Name) =="
        Push-Location $dir
        try {
            cargo build --locked --release
            if ($LASTEXITCODE -ne 0) { throw "$($mod.Name) build failed" }
        } finally {
            Pop-Location
        }
    }

} finally {
    Pop-Location
}
