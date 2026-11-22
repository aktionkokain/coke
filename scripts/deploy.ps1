# Copies the built proxy into a game folder.
# The game files are not touched; only the proxy DLL, the config file and the
# matching profiles/<exe name>/ folder are added. Names come from names.ini.
param(
    # Game folder that contains the CM executable. Omitted by default; the
    # script fails with a pointer to the expected directory when it is empty.
    [string]$GameDir = "",
    # Executable name, e.g. "CM Cold War.exe". Detected when omitted.
    [string]$Exe = "",
    [switch]$WithMods,
    [switch]$ResetConfig,
    [switch]$ReplaceProfiles,
    [switch]$Preview
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "mods.ps1")
$names = Get-CokeNames

if (-not $GameDir) {
    throw "No game folder given. Pass -GameDir with the directory that contains the Combat Mission executable, for example: scripts\deploy.ps1 -GameDir 'D:\Games\Combat Mission - Cold War'"
}

$root = Split-Path -Parent $PSScriptRoot
$dll = Join-Path $root "target\i686-pc-windows-msvc\release\$($names.proxy_dll)"
if (-not (Test-Path $dll)) {
    throw "Build the proxy first: scripts\build.ps1"
}

if (-not (Test-Path $GameDir)) {
    throw "Game folder not found: '$GameDir'"
}

if (-not $Exe) {
    $candidate = Get-ChildItem -LiteralPath $GameDir -Filter "CM *.exe" -File |
        Sort-Object Name |
        Select-Object -First 1
    if (-not $candidate) {
        throw "No 'CM *.exe' found in '$GameDir'. Pass -Exe explicitly."
    }
    $Exe = $candidate.Name
}

if ([IO.Path]::GetFileName($Exe) -ne $Exe) {
    throw "Pass an executable name for -Exe, without a directory."
}
$GameDir = (Resolve-Path -LiteralPath $GameDir).ProviderPath
$exePath = Join-Path $GameDir $Exe
if (-not (Test-Path $exePath)) {
    throw "$Exe was not found in '$GameDir'."
}

$profileSrc = Join-Path $root "$($names.profile_dir)\$Exe"
$profileDst = Join-Path $GameDir "$($names.profile_dir)\$Exe"
$configPath = Join-Path $GameDir $names.config_file
$legacyConfig = @($names.legacy_config_files -split ',' | ForEach-Object { $_.Trim() } | Where-Object { Test-Path -LiteralPath (Join-Path $GameDir $_) })
$writeConfig = $ResetConfig -or (-not (Test-Path -LiteralPath $configPath) -and $legacyConfig.Count -eq 0)
Write-Host "Deploying for $exePath"
Write-Host "Copy $dll to $GameDir"
if ($writeConfig) { Write-Host "Install default config at $configPath" }
else { Write-Host "Keep existing config in $GameDir" }
if (-not $Preview) {
    Copy-Item -LiteralPath $dll -Destination (Join-Path $GameDir $names.proxy_dll) -Force
    if ($writeConfig) {
        Copy-Item -LiteralPath (Join-Path $root $names.config_file) -Destination $configPath -Force
    }
}

if (Test-Path -LiteralPath $profileSrc) {
    foreach ($file in Get-ChildItem -LiteralPath $profileSrc -File -Filter "*.ini" -Recurse) {
        $destination = Join-Path $profileDst $file.FullName.Substring($profileSrc.Length + 1)
        if ($ReplaceProfiles -or -not (Test-Path -LiteralPath $destination)) {
            Write-Host "Copy profile $($file.FullName) to $destination"
            if (-not $Preview) {
                New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
                Copy-Item -LiteralPath $file.FullName -Destination $destination -Force
            }
        } else {
            Write-Host "Keep profile $destination"
        }
    }
} else {
    Write-Host "No bundled profiles for $Exe"
}

if ($WithMods) {
    $coke = Join-Path $GameDir $names.mod_dir
    foreach ($mod in Get-CokeMods) {
        Write-Host "Copy $($mod.Stem).dll to $coke"
        if ($mod.Config) {
            $modConfig = Join-Path $coke $mod.Config
            if (Test-Path -LiteralPath $modConfig) { Write-Host "Keep mod settings at $modConfig" }
            else { Write-Host "Install mod settings at $modConfig" }
        }
    }
    if (-not $Preview) {
        Copy-CokeMods -Destination $coke -NoConsent -PreserveConfig
    }
}
