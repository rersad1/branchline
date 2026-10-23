# Run with the configured Rust toolchain. No install or user app replacement.
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repo 'backend/target' }
if (-not [IO.Path]::IsPathRooted($targetDir)) { $targetDir = Join-Path $repo $targetDir }
cargo build --locked --release --target-dir $targetDir --manifest-path (Join-Path $repo 'backend/Cargo.toml') --bin terminal-host
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$assetsDir = Join-Path $repo 'desktop/assets'
New-Item -ItemType Directory -Path $assetsDir -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $targetDir 'release/terminal-host.exe') -Destination (Join-Path $assetsDir 'terminal-host-windows.exe')
