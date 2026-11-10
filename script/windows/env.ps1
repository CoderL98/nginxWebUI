$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Get-ConfPath([string]$Base, [string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    if ([System.IO.Path]::IsPathRooted($Value)) { return $Value }
    return [System.IO.Path]::GetFullPath((Join-Path $Base $Value))
}

function Import-NginxWebUiConf([string]$File) {
    $script:ConfFile = (Resolve-Path $File).Path
    $base = Split-Path -Parent $script:ConfFile
    Get-Content -LiteralPath $File | ForEach-Object {
        $line = $_
        $hash = $line.IndexOf('#')
        if ($hash -ge 0) { $line = $line.Substring(0, $hash) }
        $line = $line.Trim()
        if (-not $line -or $line.IndexOf('=') -lt 0) { return }
        $key, $val = $line.Split('=', 2)
        $key = $key.Trim().ToLowerInvariant().Replace('-', '_')
        $val = $val.Trim().Trim('"').Trim("'")
        if (-not $val) { return }
        switch ($key) {
            { $_ -in @('port', 'server.port', 'server_port') } { $script:ConfPort = $val }
            { $_ -in @('home', 'project.home', 'project_home') } { $script:ConfHome = Get-ConfPath $base $val }
            { $_ -in @('web_dir', 'web.dir', 'web') } { $script:ConfWeb = Get-ConfPath $base $val }
            'threads' { $script:ConfThreads = $val }
            { $_ -in @('reload_cmd', 'reload.cmd', 'reloadcmd') } { $script:ConfReloadCmd = $val }
            'bin' { $script:ConfBin = Get-ConfPath $base $val }
            'log' { $script:ConfLog = Get-ConfPath $base $val }
            'pid' { $script:ConfPid = Get-ConfPath $base $val }
        }
    }
}

$script:ConfFile = $null
$script:ConfPort = $null
$script:ConfHome = $null
$script:ConfWeb = $null
$script:ConfThreads = $null
$script:ConfReloadCmd = $null
$script:ConfBin = $null
$script:ConfLog = $null
$script:ConfPid = $null

if ($env:NGINXWEBUI_CONFIG -and (Test-Path $env:NGINXWEBUI_CONFIG)) {
    Import-NginxWebUiConf $env:NGINXWEBUI_CONFIG
} elseif (Test-Path (Join-Path $Root 'nginxwebui.conf')) {
    Import-NginxWebUiConf (Join-Path $Root 'nginxwebui.conf')
}

if (-not $env:NGINXWEBUI_PORT) { $env:NGINXWEBUI_PORT = $(if ($ConfPort) { $ConfPort } else { '8080' }) }
if (-not $env:NGINXWEBUI_HOME) { $env:NGINXWEBUI_HOME = $(if ($ConfHome) { $ConfHome } else { Join-Path $Root '.run-home' }) }
if (-not $env:NGINXWEBUI_WEB_DIR) { $env:NGINXWEBUI_WEB_DIR = $(if ($ConfWeb) { $ConfWeb } else { Join-Path $Root 'web\build' }) }

$script:Port = $env:NGINXWEBUI_PORT
$script:HomeDir = $env:NGINXWEBUI_HOME
$script:WebDir = $env:NGINXWEBUI_WEB_DIR
$script:BinDir = if ($env:NGINXWEBUI_BIN_DIR) { $env:NGINXWEBUI_BIN_DIR } else { Join-Path $HomeDir 'bin' }
$script:Bin = if ($env:NGINXWEBUI_BIN) { $env:NGINXWEBUI_BIN } elseif ($ConfBin) { $ConfBin } else { Join-Path $BinDir 'nginxwebui.exe' }
$script:PidFile = if ($env:NGINXWEBUI_PID) { $env:NGINXWEBUI_PID } elseif ($ConfPid) { $ConfPid } else { Join-Path $HomeDir 'nginxwebui.pid' }
$script:LogFile = if ($env:NGINXWEBUI_LOG) { $env:NGINXWEBUI_LOG } elseif ($ConfLog) { $ConfLog } else { Join-Path $HomeDir 'logs\nginxwebui.log' }
$script:VitePidFile = if ($env:NGINXWEBUI_VITE_PID) { $env:NGINXWEBUI_VITE_PID } else { Join-Path $HomeDir 'vite.pid' }
$script:ConfigFile = $script:ConfFile

$script:BuildDir = Join-Path $Root 'web\build'

New-Item -ItemType Directory -Force -Path (Join-Path $HomeDir 'logs'), $BinDir | Out-Null

function Sync-WebBuild {
    $index = Join-Path $BuildDir 'index.html'
    if (-not (Test-Path $index)) {
        throw "找不到构建结果: $index`n请先在仓库里成功执行 pnpm run build"
    }
    if ([string]::IsNullOrWhiteSpace($WebDir) -or $WebDir -eq '\' -or $WebDir -eq '/') {
        throw "web_dir 无效: $WebDir"
    }
    $srcRoot = (Resolve-Path (Join-Path $Root 'web')).Path
    New-Item -ItemType Directory -Force -Path $WebDir | Out-Null
    $src = (Resolve-Path $BuildDir).Path
    $dst = (Resolve-Path $WebDir).Path
    if ($dst -eq $srcRoot) {
        throw "web_dir 不能是前端源码目录 $srcRoot，请改成安装目录，例如 $HomeDir\web"
    }
    if ($src -eq $dst) {
        Write-Host "前端已在配置路径: $dst"
        return
    }
    Write-Host "同步前端 $src -> $dst"
    Get-ChildItem -LiteralPath $dst -Force | Remove-Item -Recurse -Force
    Copy-Item -Path (Join-Path $src '*') -Destination $dst -Recurse -Force
    if (-not (Test-Path (Join-Path $dst 'index.html'))) {
        throw "同步失败，没有 $dst\index.html"
    }
    Write-Host "前端已部署: $dst"
}

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
