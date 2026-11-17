# Fails when an engine address is hard-coded in src/ instead of living in a
# profile. Engine addresses are 8-digit absolute VAs (0x00xxxxxx). They belong in
# profiles/<exe>/<build>.ini as RVAs; see profiles/README.md.
#
# Comments and test modules are allowed to mention them (tests may assert on
# profile values), so those lines are skipped.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$src = Join-Path $root "src"
if (-not (Test-Path $src)) { throw "src/ not found next to scripts/" }

$address = [regex]'0x00[0-9a-fA-F]{6}'
$bad = New-Object System.Collections.Generic.List[string]

Get-ChildItem $src -Recurse -Filter *.rs | ForEach-Object {
    $file = $_
    $lines = @(Get-Content -LiteralPath $file.FullName)
    $inTest = $false
    $inBlockComment = $false

    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]

        # Everything from the first test module on is fixture data.
        if ($line -match '#\[cfg\(test\)\]') { $inTest = $true }
        if ($inTest) { continue }

        $code = $line

        if ($inBlockComment) {
            $end = $code.IndexOf('*/')
            if ($end -lt 0) { continue }
            $code = $code.Substring($end + 2)
            $inBlockComment = $false
        }

        # Remove complete /* ... */ comments, then an unterminated opener.
        $code = [regex]::Replace($code, '/\*.*?\*/', '')
        $open = $code.IndexOf('/*')
        if ($open -ge 0) {
            $code = $code.Substring(0, $open)
            $inBlockComment = $true
        }

        # Remove // comments (including /// docs).
        $slash = $code.IndexOf('//')
        if ($slash -ge 0) { $code = $code.Substring(0, $slash) }

        if ($address.IsMatch($code)) {
            $bad.Add(("{0}:{1}: {2}" -f $file.FullName, ($i + 1), $line.Trim()))
        }
    }
}

if ($bad.Count -gt 0) {
    Write-Host "Hard-coded engine addresses found in src/:" -ForegroundColor Red
    $bad | ForEach-Object { Write-Host "  $_" }
    Write-Host ""
    Write-Host "Move them into profiles/<exe>/<build>.ini as RVAs (VA - preferred_base)."
    Write-Host "See profiles/README.md."
    exit 1
}

Write-Host "No hard-coded engine addresses in src/."
exit 0
