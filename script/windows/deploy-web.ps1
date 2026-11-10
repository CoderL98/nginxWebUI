# Build the static frontend into web\build.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
Set-Location (Join-Path $Root 'web')
if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
    if (Get-Command corepack -ErrorAction SilentlyContinue) {
        corepack enable
        corepack prepare pnpm@11.24.0 --activate
    } else {
        throw '需要 pnpm（或 Node 自带的 corepack）'
    }
}
if (Test-Path 'pnpm-lock.yaml') {
    pnpm install --frozen-lockfile
} else {
    pnpm install
}
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
pnpm run build
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Sync-WebBuild
