$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'stop-web.ps1')
& (Join-Path $PSScriptRoot 'stop-server.ps1')
