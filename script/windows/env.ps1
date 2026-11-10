$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $env:NGINXWEBUI_PORT) { $env:NGINXWEBUI_PORT = '8080' }
if (-not $env:NGINXWEBUI_HOME) { $env:NGINXWEBUI_HOME = Join-Path $Root '.run-home' }
if (-not $env:NGINXWEBUI_WEB_DIR) { $env:NGINXWEBUI_WEB_DIR = Join-Path $Root 'web\build' }

$script:Port = $env:NGINXWEBUI_PORT
$script:HomeDir = $env:NGINXWEBUI_HOME
$script:WebDir = $env:NGINXWEBUI_WEB_DIR
$script:BinDir = if ($env:NGINXWEBUI_BIN_DIR) { $env:NGINXWEBUI_BIN_DIR } else { Join-Path $HomeDir 'bin' }
$script:Bin = if ($env:NGINXWEBUI_BIN) { $env:NGINXWEBUI_BIN } else { Join-Path $BinDir 'nginxwebui.exe' }
$script:PidFile = if ($env:NGINXWEBUI_PID) { $env:NGINXWEBUI_PID } else { Join-Path $HomeDir 'nginxwebui.pid' }
$script:LogFile = if ($env:NGINXWEBUI_LOG) { $env:NGINXWEBUI_LOG } else { Join-Path $HomeDir 'logs\nginxwebui.log' }
$script:VitePidFile = if ($env:NGINXWEBUI_VITE_PID) { $env:NGINXWEBUI_VITE_PID } else { Join-Path $HomeDir 'vite.pid' }

New-Item -ItemType Directory -Force -Path (Join-Path $HomeDir 'logs'), $BinDir | Out-Null

function Get-PidFromFile([string]$Path) {
    if (-not (Test-Path $Path)) { return $null }
    $raw = (Get-Content -Path $Path -ErrorAction SilentlyContinue | Select-Object -First 1)
    if (-not $raw) { return $null }
    [int]$raw
}

function Test-PidRunning([int]$ProcId) {
    $p = Get-Process -Id $ProcId -ErrorAction SilentlyContinue
    return $null -ne $p
}

function Stop-PidFile([string]$Path, [string]$Name) {
    $procId = Get-PidFromFile $Path
    if (-not $procId -or -not (Test-PidRunning $procId)) {
        if (Test-Path $Path) { Remove-Item -Force $Path }
        Write-Host "$Name 未在运行"
        return
    }
    Write-Host "停止 $Name pid=$procId"
    Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
    for ($i = 0; $i -lt 20; $i++) {
        if (-not (Test-PidRunning $procId)) { break }
        Start-Sleep -Milliseconds 200
    }
    if (Test-Path $Path) { Remove-Item -Force $Path }
}
