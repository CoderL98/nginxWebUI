$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'stop-server.ps1')
& (Join-Path $PSScriptRoot 'start-server.ps1')
