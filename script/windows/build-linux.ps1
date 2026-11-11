# Cross-compile a Linux nginxwebui binary on Windows.
# Default: x86_64-unknown-linux-musl (static, copy to Debian/Ubuntu/RHEL).
#
#   script\windows\build-linux.ps1
#   script\windows\build-linux.ps1 -Target aarch64-unknown-linux-musl
#   script\windows\build-linux.ps1 -Target x86_64-unknown-linux-gnu
#
# First run installs cargo-zigbuild and a portable Zig under
# %LOCALAPPDATA%\nginxwebui-tools\zig if zig is not already on PATH.

param(
    [string]$Target = $(if ($env:NGINXWEBUI_LINUX_TARGET) { $env:NGINXWEBUI_LINUX_TARGET } else { 'x86_64-unknown-linux-musl' })
)

$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Server = Join-Path $Root 'server'
$ZigVersion = '0.13.0'
$Tools = Join-Path $env:LOCALAPPDATA 'nginxwebui-tools'
$ZigHome = Join-Path $Tools "zig-$ZigVersion"

function Find-Zig {
    $cmd = Get-Command zig -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $local = Join-Path $ZigHome 'zig.exe'
    if (Test-Path $local) { return $local }
    return $null
}

function Install-Zig {
    New-Item -ItemType Directory -Force -Path $Tools | Out-Null
    $zipName = "zig-windows-x86_64-$ZigVersion.zip"
    $url = "https://ziglang.org/download/$ZigVersion/$zipName"
    $zipPath = Join-Path $Tools $zipName
    Write-Host "下载 Zig $ZigVersion ..."
    Invoke-WebRequest -Uri $url -OutFile $zipPath
    $extract = Join-Path $Tools 'zig-extract'
    if (Test-Path $extract) { Remove-Item -Recurse -Force $extract }
    Expand-Archive -Path $zipPath -DestinationPath $extract -Force
    $inner = Get-ChildItem $extract -Directory | Select-Object -First 1
    if (Test-Path $ZigHome) { Remove-Item -Recurse -Force $ZigHome }
    Move-Item $inner.FullName $ZigHome
    Remove-Item -Recurse -Force $extract -ErrorAction SilentlyContinue
    Remove-Item -Force $zipPath -ErrorAction SilentlyContinue
    $exe = Join-Path $ZigHome 'zig.exe'
    if (-not (Test-Path $exe)) {
        throw "Zig 解压后没有 zig.exe: $ZigHome"
    }
    return $exe
}

function Install-CargoZigbuild {
    $exe = Join-Path $env:USERPROFILE '.cargo\bin\cargo-zigbuild.exe'
    if (Test-Path $exe) { return }
    Write-Host '安装 cargo-zigbuild ...'
    cargo install cargo-zigbuild --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo install cargo-zigbuild 失败' }
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw '未找到 cargo。请先安装 Rust：https://rustup.rs'
}
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw '未找到 rustup。'
}

$zig = Find-Zig
if (-not $zig) {
    $zig = Install-Zig
}
$zigDir = Split-Path -Parent $zig
$env:PATH = "$zigDir;$env:PATH"
Write-Host "Zig: $zig"
& $zig version

Write-Host "添加 Rust target $Target"
rustup target add $Target
if ($LASTEXITCODE -ne 0) { throw "rustup target add $Target 失败" }

Install-CargoZigbuild

Set-Location $Server
Write-Host "cargo zigbuild --release --target $Target"
cargo zigbuild --release --target $Target
if ($LASTEXITCODE -ne 0) { throw '交叉编译失败' }

$built = Join-Path $Server "target\$Target\release\nginxwebui"
if (-not (Test-Path $built)) {
    throw "没有产物: $built"
}

$arch = $Target.Split('-')[0]
$outDir = Join-Path $Root "dist\linux-$arch"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$dest = Join-Path $outDir 'nginxwebui'
Copy-Item -Force $built $dest
Write-Host "Linux 产物: $dest"
$builtAgent = Join-Path $Server "target\$Target\release\nginxwebui-agent"
if (Test-Path $builtAgent) {
    $destAgent = Join-Path $outDir 'nginxwebui-agent'
    Copy-Item -Force $builtAgent $destAgent
    Write-Host "agent 产物: $destAgent（与 nginxwebui 放在同一目录，设备页一键安装才能下载）"
}
Write-Host "拷到服务器后: chmod 755 nginxwebui nginxwebui-agent && ./nginxwebui"
