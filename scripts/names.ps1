# Reads names.ini into a hashtable, so every script uses the same names as the
# DLL. Dot-source this file: `. "$PSScriptRoot\names.ps1"`.
#
# The Rust build embeds the same file through build.rs; adding a key here means
# adding it to build.rs NAME_KEYS as well.

function Get-CokeNames {
    param(
        [string]$Path = (Join-Path (Split-Path -Parent $PSScriptRoot) "names.ini")
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "names.ini not found at '$Path'"
    }

    $values = @{}
    foreach ($raw in Get-Content -LiteralPath $Path) {
        $line = ($raw -split '[#;]', 2)[0].Trim()
        if (-not $line) { continue }
        $pair = $line -split '=', 2
        if ($pair.Count -ne 2) {
            throw "names.ini: expected 'key = value', found '$line'"
        }
        $values[$pair[0].Trim().ToLowerInvariant()] = $pair[1].Trim()
    }

    foreach ($required in @(
        "product_name", "config_file", "log_file", "mod_dir", "profile_dir",
        "profiles_zip", "temp_dir", "real_dll", "proxy_dll"
    )) {
        if (-not $values.ContainsKey($required)) {
            throw "names.ini: missing required key '$required'"
        }
    }

    return [pscustomobject]$values
}
