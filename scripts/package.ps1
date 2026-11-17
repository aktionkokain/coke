# Builds the release layout in dist\:
#
#   dist\<proxy dll>        the proxy
#   dist\<config file>      the default config
#   dist\<profiles zip>     every title's profiles in one drag-and-drop zip
#   dist\mods\              the default mods with their .ini and consent files
#
# The profiles zip contains one folder per executable name
# (e.g. `CM Black Sea.exe\steam-32.ini`). The proxy creates profiles\ and
# extracts those folders into it on the first launch. Names come from names.ini.
#
# dist\mods\ mirrors the game's coke\ folder, so it can be copied in as-is.
param(
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "names.ps1")
. (Join-Path $PSScriptRoot "mods.ps1")
$names = Get-CokeNames

$root = Split-Path -Parent $PSScriptRoot
$dist = Join-Path $root "dist"
$target = Join-Path $root "target\i686-pc-windows-msvc\release"
$dll = Join-Path $target $names.proxy_dll

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot "build.ps1")
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
}

if (-not (Test-Path -LiteralPath $dll)) {
    throw "Missing $dll. Run scripts\build.ps1 first."
}

if (Test-Path -LiteralPath $dist) {
    Remove-Item -LiteralPath $dist -Recurse -Force
}
New-Item -ItemType Directory -Path $dist | Out-Null

Copy-Item $dll (Join-Path $dist $names.proxy_dll) -Force
Copy-Item (Join-Path $root $names.config_file) (Join-Path $dist $names.config_file) -Force

$modsOut = Join-Path $dist "mods"
Copy-CokeMods -Destination $modsOut

# One zip for every profile folder. The folders sit at the zip root, so the
# proxy's installer can drop them straight into profiles\.
$profiles = Join-Path $root $names.profile_dir
$staging = Join-Path $dist "staging"
New-Item -ItemType Directory -Path $staging | Out-Null
foreach ($dir in Get-ChildItem -LiteralPath $profiles -Directory) {
    $inis = Get-ChildItem -LiteralPath $dir.FullName -Filter "*.ini" -File
    if (-not $inis) { continue }

    $stagingProfile = Join-Path $staging $dir.Name
    New-Item -ItemType Directory -Path $stagingProfile -Force | Out-Null
    Copy-Item (Join-Path $dir.FullName "*.ini") $stagingProfile -Force
}

$profilesZip = Join-Path $dist $names.profiles_zip
Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $profilesZip -Force
Remove-Item -LiteralPath $staging -Recurse -Force

Write-Host ""
Write-Host "Release layout in $dist"
Get-ChildItem -LiteralPath $dist -File | ForEach-Object {
    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.Substring(0, 16)
    "{0,-24} {1,10} bytes  sha256:{2}..." -f $_.Name, $_.Length, $hash
}
Write-Host "mods\"
Get-ChildItem -LiteralPath $modsOut -File | ForEach-Object {
    $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.Substring(0, 16)
    "  {0,-24} {1,10} bytes  sha256:{2}..." -f $_.Name, $_.Length, $hash
}
