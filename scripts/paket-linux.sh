#!/usr/bin/env bash
# Baut die Linux-Pakete fuer die Veroeffentlichung:
#   dist/Fotowache-<version>-linux-<arch>.tar.gz   (fuer install.sh)
#   dist/Fotowache-<version>-linux-<arch>.AppImage (eine Datei, Doppelklick)
#
# LibRaw wird statisch eingebunden (scripts/libraw-bauen.sh) — das Programm
# braucht danach nur noch, was jede Desktop-Distribution hat: glibc, libstdc++,
# zlib und zur Laufzeit eine GPU-Schnittstelle des Systems samt X11/Wayland.
#
# Aufruf: scripts/paket-linux.sh            (baut LibRaw bei Bedarf selbst)
#         LIBRAW_DIR=<ordner> scripts/paket-linux.sh
set -euo pipefail

hier="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$hier"
export PATH="$HOME/.cargo/bin:$PATH"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"
name="Fotowache-$version-linux-$arch"

if [ -z "${LIBRAW_DIR:-}" ]; then
  export LIBRAW_DIR="$hier/target/libraw-statisch"
  [ -f "$LIBRAW_DIR/lib/libraw_r.a" ] || scripts/libraw-bauen.sh "$LIBRAW_DIR"
fi

echo "== uebersetzen ($version, $arch) =="
cargo build --release -p fotowache
binaer="${CARGO_TARGET_DIR:-target}/release/fotowache"

rm -rf "dist/$name" "dist/$name.tar.gz" "dist/$name.AppImage" dist/AppDir
mkdir -p "dist/$name"
cp "$binaer" "dist/$name/fotowache"
strip "dist/$name/fotowache" 2>/dev/null || true
cp crates/app/assets/fotowache-256.png "dist/$name/fotowache.png"
for f in LICENSE README.md THIRD-PARTY-NOTICES.md; do
  [ -f "$f" ] && cp "$f" "dist/$name/"
done
# MIME-Typen der RAW-Formate, wie sie die Typdatenbank der Desktops fuehrt.
mime_typen="image/x-adobe-dng;image/x-canon-cr2;image/x-canon-cr3;image/x-nikon-nef;image/x-sony-arw;image/x-fuji-raf;image/x-olympus-orf;image/x-panasonic-rw2;image/x-pentax-pef;image/x-dcraw;"  # Kennung: MIME-Typen
cat > "dist/$name/fotowache.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Fotowache
GenericName=RAW Photo Developer
GenericName[de]=RAW-Foto-Entwickler
Comment=Develop RAW photos from your camera
Comment[de]=Kamera-RAW-Fotos entwickeln
Exec=fotowache %F
Icon=fotowache
Terminal=false
Categories=Graphics;Photography;RasterGraphics;
MimeType=$mime_typen
StartupWMClass=fotowache
DESKTOP

echo "== Archiv =="
tar -C dist -czf "dist/$name.tar.gz" "$name"

echo "== AppImage =="
werkzeug="${APPIMAGETOOL:-}"
if [ -z "$werkzeug" ]; then
  werkzeug="$hier/target/appimagetool-$arch.AppImage"
  if [ ! -x "$werkzeug" ]; then
    case "$arch" in
      x86_64) summe=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
      aarch64) summe=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
      *) echo "kein appimagetool fuer $arch"; exit 1 ;;
    esac
    curl -fsSL -o "$werkzeug" \
      "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage"
    echo "$summe  $werkzeug" | sha256sum -c -
    chmod +x "$werkzeug"
  fi
fi
appdir="dist/AppDir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/share/icons/hicolor/256x256/apps" "$appdir/usr/share/applications"
cp "dist/$name/fotowache" "$appdir/usr/bin/fotowache"
cp "dist/$name/fotowache.png" "$appdir/usr/share/icons/hicolor/256x256/apps/fotowache.png"
cp "dist/$name/fotowache.png" "$appdir/fotowache.png"
cp "dist/$name/fotowache.desktop" "$appdir/usr/share/applications/fotowache.desktop"
cp "dist/$name/fotowache.desktop" "$appdir/fotowache.desktop"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
hier="$(dirname "$(readlink -f "$0")")"
exec "$hier/usr/bin/fotowache" "$@"
APPRUN
chmod +x "$appdir/AppRun"
# Ohne FUSE auf dem Bau-Rechner: das Werkzeug entpackt sich selbst.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$arch" "$werkzeug" --no-appstream "$appdir" "dist/$name.AppImage" >/dev/null
rm -rf "$appdir"

ls -l "dist/$name.tar.gz" "dist/$name.AppImage"
