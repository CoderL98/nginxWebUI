$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')

$existing = Get-PidFromFile $PidFile
if ($existing -and (Test-PidRunning $existing)) {
    Write-Host "后端已在运行 pid=$existing  http://127.0.0.1:$Port"
    exit 0
}

if (-not (Test-Path $Bin)) {
    Write-Error "找不到可执行文件: $Bin`n请先运行 deploy-server.ps1"
}
if (-not (Test-Path (Join-Path $WebDir 'index.html'))) {
    Write-Error "找不到前端页面: $WebDir\index.html`n请先运行 deploy-web.ps1（会把 web\build 同步到配置里的 web_dir）"
}

New-Item -ItemType Directory -Force -Path (Split-Path $LogFile) | Out-Null
$errLog = Join-Path $HomeDir 'logs\nginxwebui.err.log'
$startArgs = @(
    "--server.port=$Port",
    "--project.home=$HomeDir",
    "--web.dir=$WebDir"
)
if ($ConfigFile) { $startArgs = @("--config=$ConfigFile") + $startArgs }
$proc = Start-Process -FilePath $Bin -ArgumentList $startArgs -WorkingDirectory $HomeDir -RedirectStandardOutput $LogFile -RedirectStandardError $errLog -WindowStyle Hidden -PassThru
Set-Content -Path $PidFile -Value $proc.Id
Start-Sleep -Milliseconds 400
if (Test-PidRunning $proc.Id) {
    Write-Host "后端已启动 pid=$($proc.Id)"
    Write-Host "  http://127.0.0.1:$Port"
    if ($ConfigFile) { Write-Host "  config=$ConfigFile" }
    Write-Host "  home=$HomeDir"
    Write-Host "  log=$LogFile"
} else {
    if (Test-Path $PidFile) { Remove-Item -Force $PidFile }
    Write-Error "启动失败，查看日志: $LogFile"
}
