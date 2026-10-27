# Open Karaoke installer for Windows.
#
#   irm https://raw.githubusercontent.com/osmanahmadxai/open-karaoke/main/docs/install.ps1 | iex
#
# Downloads the latest release from GitHub and installs it for the current
# user (no admin rights). Files fetched this way aren't flagged as
# "downloaded from the internet", so SmartScreen doesn't block the installer.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # the progress bar makes downloads far slower
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$url = 'https://github.com/osmanahmadxai/open-karaoke/releases/latest/download/OpenKaraoke-Windows-Setup.exe'
$setup = Join-Path $env:TEMP 'OpenKaraoke-Setup.exe'

Write-Host '==> Downloading Open Karaoke for Windows...' -ForegroundColor Magenta
Invoke-WebRequest -Uri $url -OutFile $setup -UseBasicParsing
# Clear the internet flag in case something added one.
Unblock-File -Path $setup

Write-Host '==> Installing...' -ForegroundColor Magenta
$proc = Start-Process -FilePath $setup -ArgumentList '/S' -Wait -PassThru
Remove-Item $setup -ErrorAction SilentlyContinue
if ($proc.ExitCode -ne 0) { throw "The installer exited with code $($proc.ExitCode)." }

$dir = Join-Path $env:LOCALAPPDATA 'Open Karaoke'
$exe = Get-ChildItem -Path $dir -Filter '*.exe' -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -notmatch 'uninstall' } | Select-Object -First 1
Write-Host '==> Installed Open Karaoke. It is also in your Start menu.' -ForegroundColor Magenta
if ($exe) { Start-Process -FilePath $exe.FullName }
