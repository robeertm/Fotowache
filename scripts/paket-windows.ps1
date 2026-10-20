# Baut die Windows-Pakete fuer die Veroeffentlichung:
#   dist\Fotowache-<version>-windows-x64.zip        (portabel, fuer install.ps1)
#   dist\Fotowache-<version>-windows-x64-setup.exe  (Installer, Inno Setup)
#
# Voraussetzung: LibRaw STATISCH aus vcpkg (Triplet x64-windows-static) und
# LIBRAW_DIR zeigt auf deren installed-Ordner. Die C-Laufzeit wird ebenfalls
# statisch gelinkt (RUSTFLAGS=-C target-feature=+crt-static) — das Programm
# braucht dann kein nachinstalliertes C++-Laufzeitpaket. Einzige Ausnahme ist die
# OpenMP-Laufzeit vcomp140.dll (mehrkerniges Demosaicing): die gibt es nur als
# DLL, sie wird neben das Programm gelegt (so sieht es der Hersteller des Systems vor).
#
# Aufruf: pwsh scripts\paket-windows.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (-not $env:LIBRAW_DIR) { throw "LIBRAW_DIR ist nicht gesetzt (vcpkg installed\x64-windows-static)" }
if (-not $env:RUSTFLAGS) { $env:RUSTFLAGS = "-C target-feature=+crt-static" }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$name = "Fotowache-$version-windows-x64"
Write-Host "== uebersetzen ($version) =="
cargo build --release -p fotowache
if ($LASTEXITCODE -ne 0) { throw "cargo build fehlgeschlagen" }
$ziel = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
$exe = Join-Path $ziel "release\fotowache.exe"

$ordner = "dist\Fotowache-windows"
if (Test-Path $ordner) { Remove-Item -Recurse -Force $ordner }
New-Item -ItemType Directory -Force -Path $ordner | Out-Null
Copy-Item $exe "$ordner\fotowache.exe"
foreach ($f in "LICENSE", "README.md", "THIRD-PARTY-NOTICES.md") {
  if (Test-Path $f) { Copy-Item $f $ordner }
}

# Welche DLLs braucht das Programm? Alles, was nicht zu Windows gehoert, kommt
# aus dem Redistributable-Ordner der C++-Bauwerkzeuge daneben.
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath  # Kennung: Installationspfad
$dumpbin = Get-ChildItem "$vs\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" | Sort-Object FullName | Select-Object -Last 1  # Kennung: Installationspfad
$redist = Get-ChildItem "$vs\VC\Redist\MSVC" -Directory | Where-Object { $_.Name -match '^\d' } | Sort-Object Name | Select-Object -Last 1  # Kennung: Installationspfad
function Abhaengigkeiten($datei) {
  & $dumpbin.FullName /nologo /dependents $datei | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '\.dll$' }
}
$offen = @("$ordner\fotowache.exe")
$gesehen = @{}
while ($offen.Count -gt 0) {
  $datei = $offen[0]; $offen = $offen[1..($offen.Count)]
  foreach ($dll in (Abhaengigkeiten $datei)) {
    $klein = $dll.ToLower()
    if ($gesehen.ContainsKey($klein)) { continue }
    $gesehen[$klein] = $true
    if ($klein -match '^(vcomp|vcruntime|msvcp|concrt)') {
      $quelle = Get-ChildItem -Recurse "$($redist.FullName)\x64" -Filter $dll | Select-Object -First 1
      if (-not $quelle) { throw "$dll nicht im Redistributable-Ordner gefunden" }
      Copy-Item $quelle.FullName $ordner
      Write-Host "  mitgeliefert: $dll"
      $offen += "$ordner\$dll"
    }
  }
}

Write-Host "== ZIP =="
$zip = "dist\$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path "$ordner\*" -DestinationPath $zip

Write-Host "== Installer =="
$iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
if (-not (Test-Path $iscc)) { throw "Inno Setup 6 fehlt (choco install innosetup)" }
& $iscc /Q "/DVersion=$version" "/DQuelle=..\$ordner" installer\fotowache.iss
if ($LASTEXITCODE -ne 0) { throw "Inno Setup fehlgeschlagen" }

Get-ChildItem dist\*.zip, dist\*.exe | Format-Table Name, Length
