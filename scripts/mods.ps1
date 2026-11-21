# Default mods shipped in dist\mods\.
#
# dist\mods\ is a drag-and-drop copy of the game's coke\ folder: the mod DLL,
# its own .ini when it has one, and the consent file the proxy checks before it
# loads the mod (<consent_prefix><dll stem><consent_suffix> from names.ini).
#
# Dot-source this file: `. "$PSScriptRoot\mods.ps1"`.

. (Join-Path $PSScriptRoot "names.ps1")

function Get-CokeMods {
    # Name is the folder under mods\; Stem is the built DLL name without .dll.
    # Config is an optional .ini that lives next to the mod's Cargo.toml.
    @(
        [pscustomobject]@{ Name = "coke-fps";  Stem = "coke_fps";  Config = $null }
        [pscustomobject]@{ Name = "coke-los";  Stem = "cokelos";   Config = "cokelos.ini" }
        [pscustomobject]@{ Name = "coke-prof"; Stem = "coke_prof"; Config = "coke-prof.ini" }
    )
}

function Copy-CokeMods {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Destination
    )

    $names = Get-CokeNames
    $root = Split-Path -Parent $PSScriptRoot

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null

    # Coke Shot is an optional standalone download, never part of the default
    # drag-and-drop Coke package.
    foreach ($name in @("coke_camera.dll", "coke-camera.ini", "user_consent_coke_camera.txt")) {
        Remove-Item -LiteralPath (Join-Path $Destination $name) -Force -ErrorAction SilentlyContinue
    }

    foreach ($mod in Get-CokeMods) {
        $modDir = Join-Path $root "mods\$($mod.Name)"
        if (-not (Test-Path -LiteralPath $modDir)) { continue }

        $built = Join-Path $modDir "target\i686-pc-windows-msvc\release\$($mod.Stem).dll"
        if (-not (Test-Path -LiteralPath $built)) {
            throw "Missing $built. Run scripts\build.ps1 first."
        }
        Copy-Item -LiteralPath $built -Destination (Join-Path $Destination "$($mod.Stem).dll") -Force

        if ($mod.Config) {
            $cfg = Join-Path $modDir $mod.Config
            if (Test-Path -LiteralPath $cfg) {
                Copy-Item -LiteralPath $cfg -Destination (Join-Path $Destination $mod.Config) -Force
            }
        }

        # The proxy only checks that the file exists; the text is a reminder for
        # whoever opens the folder. Delete it to stop the mod from loading.
        $consentName = "$($names.consent_prefix)$($mod.Stem)$($names.consent_suffix)"
        $consentText = "Coke mod consent: this file lets opengl32.dll load $($mod.Stem).dll from the game's coke\ folder. Delete it to stop the mod loading."
        Set-Content -LiteralPath (Join-Path $Destination $consentName) -Value $consentText -Encoding ascii
    }
}
