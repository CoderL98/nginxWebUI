$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
Stop-PidFile $PidFile '后端'
Get-Process -Name 'nginxwebui' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
