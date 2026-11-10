$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'deploy-web.ps1')
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& (Join-Path $PSScriptRoot 'deploy-server.ps1')
Write-Host '部署完成'
