#!/usr/bin/env bash
# Baut die macOS-Pakete fuer die Veroeffentlichung:
#   dist/Fotowache-<version>-macos-<arch>.dmg  (Fenster mit „in Programme ziehen")
#   dist/Fotowache-<version>-macos-<arch>.zip  (fuer install.sh)
#
# LibRaw wird statisch eingebunden (scripts/libraw-bauen.sh) — Fotowache.app
# haengt danach nur an Systembibliotheken und laeuft ohne Homebrew.
# Signiert wird ad hoc: ohne Signatur beendet macOS das Programm beim Start
# kommentarlos. Eine Entwickler-Signatur des Systemherstellers (Notarisierung)
# gibt es nicht — deshalb fragt macOS beim ersten Oeffnen einer im Browser
# geladenen Datei einmal nach (siehe README); install.sh umgeht das.
#
# Aufruf: scripts/paket-macos.sh            (baut LibRaw bei Bedarf selbst)
#         LIBRAW_DIR=<ordner> scripts/paket-macos.sh
set -euo pipefail

hier="$(cd "$(dirname "$0")/.." && pwd)"
cd "$hier"
export PATH="$HOME/.cargo/bin:$PATH"
# Aeltere Systeme mitnehmen: macOS 12 aufwaerts. Das Ziel gilt NUR fuer den
# C-Bau von LibRaw (libraw-bauen.sh) — rustc baut ohnehin fuer 11.0 aufwaerts,
# und mit gesetztem MACOSX_DEPLOYMENT_TARGET liessen sich frisch gebaute
# Proc-Macros auf neueren macOS-Versionen nicht mehr laden (E0463).
mindest_macos="12.0"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"           # arm64 oder x86_64
name="Fotowache-$version-macos-$arch"

if [ -z "${LIBRAW_DIR:-}" ]; then
  export LIBRAW_DIR="$hier/target/libraw-statisch"
  [ -f "$LIBRAW_DIR/lib/libraw_r.a" ] || MACOSX_DEPLOYMENT_TARGET="$mindest_macos" scripts/libraw-bauen.sh "$LIBRAW_DIR"
fi

echo "== uebersetzen ($version, $arch) =="
env -u MACOSX_DEPLOYMENT_TARGET cargo build --release -p fotowache
binaer="${CARGO_TARGET_DIR:-target}/release/fotowache"

app="dist/Fotowache.app"
rm -rf "$app" "dist/$name.dmg" "dist/$name.zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binaer" "$app/Contents/MacOS/Fotowache"
strip -x "$app/Contents/MacOS/Fotowache" 2>/dev/null || true

# Kein Homebrew-Verweis darf uebrig sein — sonst startet das Paket woanders nicht.
if otool -L "$app/Contents/MacOS/Fotowache" | tail -n +2 | grep -Ev '^\s*(/usr/lib|/System)/'; then
  echo "FEHLER: Programm haengt an Bibliotheken ausserhalb des Systems (siehe oben)"
  exit 1
fi

echo "== Symbol =="
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 scripts/make_icon.py "$tmp/icon.png" 1024 >/dev/null
mkdir -p "$tmp/Fotowache.iconset"
for s in 16 32 64 128 256 512; do
  sips -z $s $s "$tmp/icon.png" --out "$tmp/Fotowache.iconset/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$tmp/icon.png" --out "$tmp/Fotowache.iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$tmp/Fotowache.iconset" -o "$app/Contents/Resources/Fotowache.icns"

echo "== Info.plist =="
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"> <!-- Kennung: Dokumenttyp -->
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Fotowache</string>
  <key>CFBundleDisplayName</key><string>Fotowache</string>
  <key>CFBundleExecutable</key><string>Fotowache</string>
  <key>CFBundleIdentifier</key><string>io.github.robeertm.fotowache</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>Fotowache</string>
  <key>LSMinimumSystemVersion</key><string>$mindest_macos</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.photography</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>RAW Photo</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.camera-raw-image</string>
        <string>com.adobe.raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.fujifilm.raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.canon.cr2-raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.canon.cr3-raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.nikon.raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.sony.arw-raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.olympus.raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.panasonic.rw2-raw-image</string> <!-- Kennung: Typkennung -->
        <string>com.pentax.raw-image</string> <!-- Kennung: Typkennung -->
      </array>
    </dict>
  </array>
</dict>
</plist>
PLIST

echo "== signieren (ad hoc) =="
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

echo "== ZIP =="
( cd dist && ditto -c -k --keepParent Fotowache.app "$name.zip" )

echo "== DMG =="
inhalt="$tmp/dmg"
mkdir -p "$inhalt"
cp -R "$app" "$inhalt/"
ln -s /Applications "$inhalt/Applications"
hdiutil create -volname "Fotowache $version" -srcfolder "$inhalt" -ov -format UDZO "dist/$name.dmg" >/dev/null

ls -l "dist/$name.dmg" "dist/$name.zip"
