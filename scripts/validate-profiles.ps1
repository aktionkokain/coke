# Use the proxy's profile parser without launching or modifying a game.
param([string]$Path = (Join-Path (Split-Path -Parent $PSScriptRoot) "profiles"))
$ErrorActionPreference = "Stop"
$previous = $env:COKE_VALIDATE_PROFILES
$env:COKE_VALIDATE_PROFILES = (Resolve-Path -LiteralPath $Path).ProviderPath
Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    cargo test --locked profile::tests::offline_profile_validation -- --exact --nocapture
    if ($LASTEXITCODE -ne 0) { throw "Profile validation failed" }
} finally {
    Pop-Location
    $env:COKE_VALIDATE_PROFILES = $previous
}
