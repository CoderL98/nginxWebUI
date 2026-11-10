# Build the Rust backend, copy the exe, restart the process.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
Set-Location (Join-Path $Root 'server')
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Copy-Item -Force (Join-Path $Root 'server\target\release\nginxwebui.exe') $Bin
Write-Host "后端二进制: $Bin"
& (Join-Path $PSScriptRoot 'restart-server.ps1')
