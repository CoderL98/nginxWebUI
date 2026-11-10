# Optional Vite dev server. Production does not need this.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')

$existing = Get-PidFromFile $VitePidFile
if ($existing -and (Test-PidRunning $existing)) {
    Write-Host "前端开发服务已在运行 pid=$existing  http://127.0.0.1:5173"
    exit 0
}

Set-Location (Join-Path $Root 'web')
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    if (Get-Command corepack -ErrorAction SilentlyContinue) {
        corepack enable
        corepack prepare pnpm@11.24.0 --activate
    } else {
        throw '需要 pnpm（或 Node 自带的 corepack）'
    }
}
if (-not (Test-Path 'node_modules')) { pnpm install }
$viteLog = Join-Path $HomeDir 'logs\vite.log'
$viteErr = Join-Path $HomeDir 'logs\vite.err.log'
$pnpm = (Get-Command pnpm.cmd -ErrorAction SilentlyContinue).Source
if (-not $pnpm) { $pnpm = (Get-Command pnpm -ErrorAction SilentlyContinue).Source }
if (-not $pnpm) { $pnpm = 'pnpm' }
$proc = Start-Process -FilePath $pnpm -ArgumentList @('run', 'dev') -WorkingDirectory (Join-Path $Root 'web') -RedirectStandardOutput $viteLog -RedirectStandardError $viteErr -WindowStyle Hidden -PassThru
Set-Content -Path $VitePidFile -Value $proc.Id
Start-Sleep -Milliseconds 800
Write-Host "前端开发服务已启动 pid=$($proc.Id)"
Write-Host "  http://127.0.0.1:5173  (API 代理到 :$Port，请同时 start-server.ps1)"
Write-Host "  log=$viteLog"
