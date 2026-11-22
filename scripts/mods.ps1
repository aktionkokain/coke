# Shared manifest for build, packaging and deployment.
. (Join-Path $PSScriptRoot "names.ps1")

function Get-CokeMods {
    # Name is the folder under mods\; Stem is the built DLL name without .dll.
    # Config is an optional .ini that lives next to the mod's Cargo.toml.
    @(
        [pscustomobject]@{ Name = "coke-fps";  Stem = "coke_fps";  Config = $null; ShipConsent = $true }
        [pscustomobject]@{ Name = "coke-range"; Stem = "coke_range"; Config = $null; ShipConsent = $true }
        [pscustomobject]@{ Name = "coke-prof"; Stem = "coke_prof"; Config = "coke-prof.ini"; ShipConsent = $true }
        [pscustomobject]@{ Name = "coke-camera"; Stem = "coke_camera"; Config = "coke-camera.ini"; ShipConsent = $false }
    )
}

function Copy-CokeMods {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Destination,
        [switch]$NoConsent,
        [switch]$PreserveConfig
    )

    $names = Get-CokeNames
    $root = Split-Path -Parent $PSScriptRoot

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null

    foreach ($mod in Get-CokeMods) {
        $modDir = Join-Path $root "mods\$($mod.Name)"

        $built = Join-Path $modDir "target\i686-pc-windows-msvc\release\$($mod.Stem).dll"
        if (-not (Test-Path -LiteralPath $built)) {
            throw "Missing $built. Run scripts\build.ps1 first."
        }
        Copy-Item -LiteralPath $built -Destination (Join-Path $Destination "$($mod.Stem).dll") -Force

        if ($mod.Config) {
            $cfg = Join-Path $modDir $mod.Config
            if (-not $PreserveConfig -or -not (Test-Path -LiteralPath (Join-Path $Destination $mod.Config))) {
                Copy-Item -LiteralPath $cfg -Destination (Join-Path $Destination $mod.Config) -Force
            }
        }

        if ($NoConsent -or -not $mod.ShipConsent) { continue }

        # The proxy only checks that the file exists; the text is a reminder for
        # whoever opens the folder. Delete it to stop the mod from loading.
        $consentName = "$($names.consent_prefix)$($mod.Stem)$($names.consent_suffix)"
        $consentText = "Coke mod consent: this file lets opengl32.dll load $($mod.Stem).dll from the game's coke\ folder. Delete it to stop the mod loading."
        Set-Content -LiteralPath (Join-Path $Destination $consentName) -Value $consentText -Encoding ascii
    }
}
