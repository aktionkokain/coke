# Builds the release layout in dist\:
#
#   dist\<proxy dll>        the proxy
#   dist\<config file>      the default config
#   dist\<profiles zip>     every title's profiles in one drag-and-drop zip
#   dist\coke-mods.zip      all bundled mods
#   dist\mods\              the unpacked mod bundle
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

$root = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
$published = Join-Path $root "dist"
$work = Join-Path $root (".coke-package-" + [guid]::NewGuid().ToString("N"))
$dist = Join-Path $work "release"
$target = Join-Path $root "target\i686-pc-windows-msvc\release"
$dll = Join-Path $target $names.proxy_dll

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot "build.ps1")
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
}

if (-not (Test-Path -LiteralPath $dll)) {
    throw "Missing $dll. Run scripts\build.ps1 first."
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
New-Item -ItemType Directory -Path $dist -Force | Out-Null
$backup = Join-Path $work "previous"
try {

    function Assert-CokeZip {
        param([string]$Path, [string[]]$Expected)
        $zip = [IO.Compression.ZipFile]::OpenRead($Path)
        try {
            $actual = @($zip.Entries | Where-Object Name | ForEach-Object { $_.FullName.Replace('\', '/') })
            if ($actual.Count -ne $Expected.Count -or (Compare-Object $Expected $actual -CaseSensitive)) {
                throw "Unexpected contents in $Path"
            }
        } finally {
            $zip.Dispose()
        }
    }

    Copy-Item $dll (Join-Path $dist $names.proxy_dll) -Force
    Copy-Item (Join-Path $root $names.config_file) (Join-Path $dist $names.config_file) -Force

    $modsOut = Join-Path $dist "mods"
    Copy-CokeMods -Destination $modsOut
    $expected = @(
        foreach ($mod in Get-CokeMods) {
            "$($mod.Stem).dll"
            if ($mod.Config) { $mod.Config }
            if ($mod.ShipConsent) { "$($names.consent_prefix)$($mod.Stem)$($names.consent_suffix)" }
        }
    )
    $modsZip = Join-Path $dist "coke-mods.zip"
    Compress-Archive -Path (Join-Path $modsOut "*") -DestinationPath $modsZip
    Assert-CokeZip $modsZip $expected

    # One zip for every profile folder. The folders sit at the zip root, so the
    # proxy's installer can drop them straight into profiles\.
    $profiles = Join-Path $root $names.profile_dir
    $staging = Join-Path $work "profiles"
    New-Item -ItemType Directory -Path $staging | Out-Null
    foreach ($file in Get-ChildItem -LiteralPath $profiles -Filter "*.ini" -File -Recurse) {
        $relative = $file.FullName.Substring($profiles.Length + 1)
        if ($relative -notmatch '[\\/]') { continue }
        $destination = Join-Path $staging $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
        Copy-Item -LiteralPath $file.FullName -Destination $destination
    }

    $profilesZip = Join-Path $dist $names.profiles_zip
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $profilesZip -Force
    Assert-CokeZip $profilesZip @(Get-ChildItem -LiteralPath $staging -File -Recurse | ForEach-Object {
        $_.FullName.Substring($staging.Length + 1).Replace('\', '/')
    })

    Get-ChildItem -LiteralPath $dist -File | Sort-Object Name | ForEach-Object {
        "{0}  {1}" -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name
    } | Set-Content -LiteralPath (Join-Path $dist "SHA256SUMS.txt") -Encoding ascii

    if (Test-Path -LiteralPath $published) {
        if ((Get-Item -LiteralPath $published -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Release destination is a reparse point: $published"
        }
        [IO.Directory]::Move($published, $backup)
    }
    try {
        [IO.Directory]::Move($dist, $published)
    } catch {
        if (Test-Path -LiteralPath $backup) { [IO.Directory]::Move($backup, $published) }
        throw
    }
    Write-Host "Verified release layout in $published"
} finally {
    $resolvedWork = (Resolve-Path -LiteralPath $work).ProviderPath
    $workName = [IO.Path]::GetFileName($resolvedWork)
    if ([IO.Path]::GetDirectoryName($resolvedWork) -eq $root -and $workName -match '^\.coke-package-[0-9a-f]{32}$') {
        if ((Test-Path -LiteralPath $dist) -and (Test-Path -LiteralPath $backup)) {
            Write-Warning "Previous release is still at $backup"
        } else {
            Remove-Item -LiteralPath $resolvedWork -Recurse -Force
        }
    }
}
