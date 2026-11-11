# nginxWebUI agent installer, served by the panel at /agent/install.ps1.
#
#   irm https://panel.example/agent/install.ps1 -OutFile $env:TEMP\nginxwebui-agent-install.ps1
#   powershell -ExecutionPolicy Bypass -File $env:TEMP\nginxwebui-agent-install.ps1 -Url https://panel.example -Token nwj_...
#
# Downloads nginxwebui-agent.exe, writes agent.json, and installs a real Windows
# Service (the exe implements Service Control Manager, not a bare console).

param(
    [Parameter(Mandatory = $false)][string]$Url = "",
    [Parameter(Mandatory = $false)][string]$Token = "",
    [string]$HomeDir = "",
    [string]$TlsCa = "",
    [string]$TlsFingerprint = "",
    [switch]$NoUpgrade,
    [switch]$Foreground
)

$ErrorActionPreference = "Stop"
$Panel = "__PANEL__"

function Fail($msg) {
    Write-Error $msg
    exit 1
}

# Parse leftover `-Url` / `--url` style args if the script was invoked via iex.
if (-not $Url -or -not $Token) {
    $argv = @($args)
    for ($i = 0; $i -lt $argv.Count; $i++) {
        switch -Regex ($argv[$i]) {
            "^--url$|^-Url$" { $Url = $argv[$i + 1]; $i++; break }
            "^--token$|^-Token$" { $Token = $argv[$i + 1]; $i++; break }
            "^--home$|^-HomeDir$" { $HomeDir = $argv[$i + 1]; $i++; break }
            "^--tls-ca$" { $TlsCa = $argv[$i + 1]; $i++; break }
            "^--tls-fingerprint$" { $TlsFingerprint = $argv[$i + 1]; $i++; break }
            "^--no-upgrade$" { $NoUpgrade = $true; break }
            "^--foreground$" { $Foreground = $true; break }
        }
    }
}

if (-not $Url) { $Url = $Panel }
if (-not $Url -or -not $Token) {
    Fail "Usage: install.ps1 -Url https://panel.example -Token <join-token>"
}

$Url = $Url.TrimEnd("/")
if ($Url -notmatch "^https://") {
    if ($Url -match "^http://") {
        $hostName = ([System.Uri]$Url).Host
        $loopback = ($hostName -eq "localhost") -or $hostName.StartsWith("127.") -or $hostName -eq "::1"
        if (-not $loopback -and -not $env:NGINXWEBUI_ALLOW_HTTP) {
            Fail "Refusing plain http:// panel URL. Set NGINXWEBUI_ALLOW_HTTP=1 only on a trusted network."
        }
    } else {
        Fail "Url must start with https:// (or http:// on a trusted network)"
    }
}

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -eq "AMD64" -or $arch -eq "x86_64") { $archTag = "x86_64" }
elseif ($arch -eq "ARM64") { $archTag = "aarch64" }
else { $archTag = "x86_64" }

$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $HomeDir) {
    if ($isAdmin -and -not $Foreground) {
        $HomeDir = Join-Path $env:ProgramData "nginxwebui-agent"
    } else {
        $base = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { $env:USERPROFILE }
        $HomeDir = Join-Path $base "nginxwebui-agent"
    }
}

$destDir = if ($isAdmin -and -not $Foreground) { Join-Path $env:ProgramFiles "nginxwebui-agent" } else { (Get-Location).Path }
New-Item -ItemType Directory -Force -Path $destDir | Out-Null
New-Item -ItemType Directory -Force -Path $HomeDir | Out-Null
$exe = Join-Path $destDir "nginxwebui-agent.exe"

$download = "$Url/agent/download?os=windows&arch=$archTag"
Write-Host "Downloading $download"
try {
    Invoke-WebRequest -Uri $download -OutFile $exe -UseBasicParsing
} catch {
    Fail "Download failed: $_"
}
if (-not (Test-Path $exe) -or (Get-Item $exe).Length -lt 1024) {
    Fail "Downloaded agent is missing or too small. Place windows-x86_64/nginxwebui-agent.exe under the panel's agents/ directory."
}

$joinArgs = @("join", "--url", $Url, "--token", $Token, "--home", $HomeDir)
if ($TlsCa) { $joinArgs += @("--tls-ca", $TlsCa) }
if ($TlsFingerprint) { $joinArgs += @("--tls-fingerprint", $TlsFingerprint) }
if ($NoUpgrade) { $joinArgs += "--no-upgrade" }
& $exe @joinArgs
if ($LASTEXITCODE -ne 0) {
    Fail "join failed with exit code $LASTEXITCODE"
}

if ($Foreground -or -not $isAdmin) {
    Write-Host "Starting in the foreground. For a Windows Service, re-run this script from an elevated PowerShell."
    $runArgs = @("run", "--home", $HomeDir)
    if ($NoUpgrade) { $runArgs += "--no-upgrade" }
    & $exe @runArgs
    exit $LASTEXITCODE
}

$svcName = "nginxwebui-agent"
$binPath = "`"$exe`" service --home `"$HomeDir`""
$existing = Get-Service -Name $svcName -ErrorAction SilentlyContinue
if ($existing) {
    Stop-Service -Name $svcName -ErrorAction SilentlyContinue
    sc.exe delete $svcName | Out-Null
    Start-Sleep -Seconds 1
}
New-Service -Name $svcName -BinaryPathName $binPath -DisplayName "nginxWebUI Agent" -Description "Outbound nginxWebUI agent: long-polls the panel and manages the local nginx." -StartupType Automatic | Out-Null
# Automatic recovery
sc.exe failure $svcName reset= 60 actions= restart/5000/restart/5000/restart/5000 | Out-Null
Start-Service -Name $svcName
Write-Host "Installed and started service $svcName."
Write-Host "The service account must be able to write the nginx conf/ssl directories (and reload nginx)."
Write-Host "Certificates require Git for Windows (Git\\bin\\bash.exe) or WSL so the pinned acme.sh can run."
