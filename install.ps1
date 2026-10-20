# Fotowache installer for Windows (current user, no administrator rights).
#
#   irm https://raw.githubusercontent.com/robeertm/Fotowache/main/install.ps1 | iex
#
# Downloads the latest release, checks its SHA-256 checksum, installs it to
# %LOCALAPPDATA%\Programs\Fotowache and adds a Start menu entry.
# Uninstall: run the same command with -Uninstall, e.g.
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/robeertm/Fotowache/main/install.ps1))) -Uninstall
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # makes Invoke-WebRequest much faster

$repo = 'robeertm/Fotowache'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Fotowache'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Fotowache.lnk' # Kennung: Systempfad

if ($Uninstall) {
  Get-Process fotowache -ErrorAction SilentlyContinue | Stop-Process -Force
  Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
  Remove-Item -Force $startMenu -ErrorAction SilentlyContinue
  Write-Host 'Fotowache removed. Your photos and their .fotowache.json edit files were not touched.'
  return
}

if (-not [Environment]::Is64BitOperatingSystem) { throw 'Fotowache needs 64-bit Windows.' }

Write-Host 'Looking up the latest Fotowache release ...'
$release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
$asset = $release.assets | Where-Object { $_.name -like '*-windows-x64.zip' } | Select-Object -First 1
if (-not $asset) { throw 'The latest release has no Windows package.' }
$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' } | Select-Object -First 1

$tmp = Join-Path ([IO.Path]::GetTempPath()) ("fotowache-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  $zip = Join-Path $tmp $asset.name
  Write-Host "Downloading $($asset.name) ..."
  Invoke-WebRequest $asset.browser_download_url -OutFile $zip -UseBasicParsing
  if ($sums) {
    $lines = (Invoke-WebRequest $sums.browser_download_url -UseBasicParsing).Content -split "`n"
    $line = $lines | Where-Object { $_ -match [regex]::Escape($asset.name) + '\s*$' } | Select-Object -First 1
    $expected = ($line -split '\s+')[0].ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if (-not $expected -or $expected -ne $actual) { throw "Checksum mismatch for $($asset.name)" }
    Write-Host 'Checksum OK.'
  }
  Get-Process fotowache -ErrorAction SilentlyContinue | Stop-Process -Force
  Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
  Expand-Archive $zip -DestinationPath $dest -Force

  $shell = New-Object -ComObject WScript.Shell
  $link = $shell.CreateShortcut($startMenu)
  $link.TargetPath = Join-Path $dest 'fotowache.exe'
  $link.WorkingDirectory = $dest
  $link.Save()
  Write-Host "Fotowache $($release.tag_name) installed to $dest"
  Write-Host 'Start it from the Start menu (Fotowache).'
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
