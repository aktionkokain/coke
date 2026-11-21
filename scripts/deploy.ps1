# Copies the built proxy into a game folder.
# The game files are not touched; only the proxy DLL, the config file and the
# matching profiles/<exe name>/ folder are added. Names come from names.ini.
param(
    # Game folder that contains the CM executable. Omitted by default; the
    # script fails with a pointer to the expected directory when it is empty.
    [string]$GameDir = "",
    # Executable name, e.g. "CM Cold War.exe". Detected when omitted.
    [string]$Exe = "",
    [switch]$WithMods
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "names.ps1")
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

$exePath = Join-Path $GameDir $Exe
if (-not (Test-Path $exePath)) {
    throw "$Exe was not found in '$GameDir'."
}

Copy-Item $dll (Join-Path $GameDir $names.proxy_dll) -Force

# The config carries the update-check state in its [state] section. Keep those
# key/values across deployments instead of resetting the player's answer, and
# make sure the installed file has exactly one [state] section.
$configPath = Join-Path $GameDir $names.config_file
$state = [ordered]@{}
if (Test-Path -LiteralPath $configPath) {
    $inState = $false
    foreach ($line in Get-Content -LiteralPath $configPath) {
        $trimmed = $line.Trim()
        if ($trimmed -match '^\[(.+)\]$') {
            $inState = ($Matches[1].Trim() -ieq 'state')
            continue
        }
        if ($inState -and $trimmed -match '^([A-Za-z0-9_]+)\s*=\s*(.*)$') {
            $state[$Matches[1]] = $Matches[2].Trim()
        }
    }
}
Copy-Item (Join-Path $root $names.config_file) $configPath -Force
if ($state.Count -gt 0) {
    $kept = @()
    $inState = $false
    foreach ($line in Get-Content -LiteralPath $configPath) {
        $trimmed = $line.Trim()
        if ($trimmed -match '^\[(.+)\]$') {
            $inState = ($Matches[1].Trim() -ieq 'state')
            if ($inState) { continue }
        }
        if ($inState) { continue }
        $kept += $line
    }
    while ($kept.Count -gt 0 -and $kept[-1].Trim() -eq '') {
        if ($kept.Count -eq 1) { $kept = @(); break }
        $kept = $kept[0..($kept.Count - 2)]
    }
    $block = @('', '[state]', '# Written by Coke. Delete this section to see the update prompt again.')
    foreach ($key in $state.Keys) { $block += "$key=$($state[$key])" }
    $kept += $block
    [IO.File]::WriteAllText($configPath, (($kept -join [Environment]::NewLine) + [Environment]::NewLine))
    Write-Host "Kept the [state] section in $($names.config_file)"
}
Write-Host "Installed $($names.proxy_dll) and $($names.config_file) in $GameDir"

# The profile for this executable. CMBS also ships one embedded, but copying the
# file next to the game makes the selected identity visible and easy to inspect.
$profileSrc = Join-Path $root "$($names.profile_dir)\$Exe"
if (Test-Path $profileSrc) {
    $profileDst = Join-Path $GameDir "$($names.profile_dir)\$Exe"
    New-Item -ItemType Directory -Path $profileDst -Force | Out-Null
    Copy-Item (Join-Path $profileSrc "*.ini") $profileDst -Force
    Write-Host "Installed profile(s) in $($names.profile_dir)\$Exe"
} else {
    Write-Host "No profile for '$Exe' in $($names.profile_dir)\; the game will run in safe mode (no engine writes)."
}

$coke = Join-Path $GameDir $names.mod_dir
New-Item -ItemType Directory -Path $coke -Force | Out-Null

if ($WithMods) {
    $mods = @()
    if ($WithMods) {
        $mods += @(
            @{ Dir = "coke-fps"; Dll = "coke_fps.dll"; Ini = $null },
            @{ Dir = "coke-los"; Dll = "cokelos.dll"; Ini = "cokelos.ini" },
            @{ Dir = "coke-range"; Dll = "coke_range.dll"; Ini = $null },
            @{ Dir = "coke-prof"; Dll = "coke_prof.dll"; Ini = "coke-prof.ini" },
            @{ Dir = "coke-camera"; Dll = "coke_camera.dll"; Ini = "coke-camera.ini" }
        )
    }
    foreach ($mod in $mods) {
        $modDll = Join-Path $root "mods\$($mod.Dir)\target\i686-pc-windows-msvc\release\$($mod.Dll)"
        if (Test-Path $modDll) {
            Copy-Item $modDll $coke -Force
            $stem = [System.IO.Path]::GetFileNameWithoutExtension($mod.Dll)
            Write-Host "Copied $($mod.Dll) to coke\"
            Write-Host "  to enable it, create coke\user_consent_$stem.txt (empty is fine)"
            if ($mod.Ini) {
                $modIni = Join-Path $root "mods\$($mod.Dir)\$($mod.Ini)"
                if (Test-Path -LiteralPath $modIni) {
                    Copy-Item $modIni $coke -Force
                }
            }
        }
    }
    $profIni = Join-Path $root "mods\coke-prof\coke-prof.ini"
    if (Test-Path $profIni) {
        Copy-Item $profIni (Join-Path $GameDir "coke-prof.ini") -Force
        Copy-Item $profIni (Join-Path $coke "coke-prof.ini") -Force
    }
}

Write-Host ""
Write-Host "Uninstall: delete $($names.proxy_dll), $($names.config_file) and $($names.profile_dir)\ from the game folder."
