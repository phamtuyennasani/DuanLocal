$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$AppDir = Join-Path $Root 'crates/app'
$Target = if ($env:DEVSTACK_TARGET) { $env:DEVSTACK_TARGET } else { 'x86_64-pc-windows-msvc' }

if ($env:OS -ne 'Windows_NT') {
    throw 'This script must run on Windows.'
}
if ($Target -ne 'x86_64-pc-windows-msvc') {
    throw "Unsupported Windows target: $Target"
}

$installedTargets = rustup target list --installed
if ($installedTargets -notcontains $Target) {
    throw "Rust target $Target is not installed. Run: rustup target add $Target"
}

$cargoTauri = Get-Command cargo-tauri -ErrorAction SilentlyContinue
if (-not $cargoTauri) {
    cargo tauri --version *> $null
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri CLI is required. Install it with: cargo install tauri-cli --version '^2'"
    }
}

$resourceDir = Join-Path $AppDir 'resources'
if (Test-Path $resourceDir) {
    Remove-Item $resourceDir -Recurse -Force
}

Write-Host "Building Windows NSIS installer"
Push-Location $AppDir
try {
    cargo tauri build --target $Target --bundles nsis --config tauri.windows.conf.json
    if ($LASTEXITCODE -ne 0) { throw 'Tauri Windows bundle failed' }
}
finally {
    Pop-Location
}

$BundleDir = Join-Path $Root 'target/release/bundle/nsis'
$Installer = Get-ChildItem $BundleDir -Filter '*.exe' -File | Select-Object -First 1
if (-not $Installer) { throw "Missing NSIS installer in $BundleDir" }

$Hash = (Get-FileHash $Installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
"$Hash  $($Installer.Name)" | Set-Content "$($Installer.FullName).sha256" -Encoding ascii

Write-Host ''
Write-Host 'Packaging complete:'
Write-Host "  $($Installer.FullName)"
Write-Host "  $($Installer.FullName).sha256"
Get-FileHash $Installer.FullName -Algorithm SHA256
