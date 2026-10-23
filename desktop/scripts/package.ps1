[CmdletBinding()]
param([string] $Distro = 'Ubuntu')
$ErrorActionPreference = 'Stop'
$desktopDir = Split-Path -Parent $PSScriptRoot
$repoDir = Split-Path -Parent $desktopDir
Push-Location $repoDir
try {
    # Build-time tools only. No default-distro or developer-home runtime fallback.
    wsl.exe --distribution $Distro --exec bash -l scripts/build-d14-worker.sh
    if ($LASTEXITCODE -ne 0) { throw 'Linux helper release build failed.' }
    & (Join-Path $repoDir 'scripts/build-d4-terminal-host.ps1')
    if ($LASTEXITCODE -ne 0) { throw 'Windows terminal host build failed.' }
    npm.cmd --prefix frontend ci
    if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency installation failed.' }
    npm.cmd --prefix frontend run build
    if ($LASTEXITCODE -ne 0) { throw 'Frontend production build failed.' }
    npm.cmd --prefix desktop ci
    if ($LASTEXITCODE -ne 0) { throw 'Tauri CLI dependency installation failed.' }
    Push-Location $desktopDir
    try {
        npm.cmd run tauri -- build --ci --bundles nsis -- --locked
        if ($LASTEXITCODE -ne 0) { throw 'Desktop installer build failed.' }
    } finally { Pop-Location }
} finally { Pop-Location }
