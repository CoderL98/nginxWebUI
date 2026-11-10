$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'stop-web.ps1')
& (Join-Path $PSScriptRoot 'start-web.ps1')
