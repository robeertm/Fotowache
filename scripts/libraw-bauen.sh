#!/usr/bin/env bash
# Baut LibRaw (und libjpeg-turbo fuer verlustbehaftete DNGs) als STATISCHE
# Bibliotheken — fuer Pakete, die auf fremden Rechnern ohne Homebrew/apt-LibRaw
# laufen sollen (macOS und Linux; Windows nimmt vcpkg, siehe Bauablauf).
#
# Warum selbst bauen statt die Systembibliothek: die Kamera-Unterstuetzung
# haengt an der LibRaw-Version. Ubuntu 24.04 liefert 0.21 — dort fehlen z. B.
# die Testkamera (40 MP) und andere Kameras ab 2024. Eine feste, gepruefte Version
# fuer alle Systeme heisst: jede Kamera verhaelt sich ueberall gleich.
#
# Aufruf: scripts/libraw-bauen.sh <zielordner>
# Danach: PKG_CONFIG_PATH=<zielordner>/lib/pkgconfig LIBRAW_STATIC=1 cargo build --release
set -euo pipefail

LIBRAW_VERSION=0.22.2
LIBRAW_SHA256=de86b035655accff8d4010f1a221fdf50d353cb7b1422ba26f14a0db92612cfa
JPEG_VERSION=3.1.4.1
JPEG_SHA256=ecae8008e2cc9ade2f2c1bb9d5e6d4fb73e7c433866a056bd82980741571a022

ziel="${1:?Aufruf: libraw-bauen.sh <zielordner>}"
mkdir -p "$ziel"
ziel="$(cd "$ziel" && pwd)"
arbeit="$(mktemp -d)"
trap 'rm -rf "$arbeit"' EXIT

pruefen() { # datei summe
  if command -v sha256sum >/dev/null; then
    echo "$2  $1" | sha256sum -c -
  else
    echo "$2  $1" | shasum -a 256 -c -
  fi
}

kerne="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
# macOS: fuer aeltere Systeme bauen (sonst traegt jede Objektdatei die
# Version des Bau-Rechners, und das Programm verlangte sie ebenfalls).
if [ "$(uname -s)" = "Darwin" ]; then
  export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-12.0}"
fi

echo "== libjpeg-turbo $JPEG_VERSION =="
curl -fsSL -o "$arbeit/jpeg.tar.gz" \
  "https://github.com/libjpeg-turbo/libjpeg-turbo/releases/download/$JPEG_VERSION/libjpeg-turbo-$JPEG_VERSION.tar.gz"
pruefen "$arbeit/jpeg.tar.gz" "$JPEG_SHA256"
tar xzf "$arbeit/jpeg.tar.gz" -C "$arbeit"
# Ohne SIMD: sonst braucht x86 den Assembler nasm. LibRaw dekodiert nur die
# seltenen verlustbehafteten DNGs damit — Geschwindigkeit spielt keine Rolle.
cmake -S "$arbeit/libjpeg-turbo-$JPEG_VERSION" -B "$arbeit/jpeg-bau" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$ziel" \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DENABLE_SHARED=OFF -DENABLE_STATIC=ON -DWITH_SIMD=OFF -DWITH_TURBOJPEG=OFF -DWITH_JPEG8=ON \
  >/dev/null
cmake --build "$arbeit/jpeg-bau" -j "$kerne" >/dev/null
cmake --install "$arbeit/jpeg-bau" >/dev/null

echo "== OpenMP =="
# OpenMP verteilt das Demosaicing auf alle Kerne. Ohne brauchte ein
# 26-MP-RAF-Foto mit 6x6-Farbmuster 16,9 s statt 2,4 s — gemessen. Die Laufzeit wird
# statisch mitgelinkt, damit das Paket nichts nachinstallieren muss.
omp_flags=""
omp_libs=""
openmp_schalter="--disable-openmp"
case "$(uname -s)" in
  Darwin)
    # Der clang von macOS bringt keine OpenMP-Laufzeit mit — LLVMs libomp (Homebrew).
    omp="${LIBOMP_DIR:-$(brew --prefix libomp 2>/dev/null || true)}"
    if [ -n "$omp" ] && [ -f "$omp/lib/libomp.a" ]; then
      cp "$omp/lib/libomp.a" "$ziel/lib/"
      mkdir -p "$ziel/include"
      cp "$omp/include/"*.h "$ziel/include/"
      # LibRaw schaltet OpenMP fuer die thread-sichere Ausgabe auf macOS nur
      # mit LIBRAW_FORCE_OPENMP ein (so baut es auch Homebrew).
      omp_flags="-Xpreprocessor -fopenmp -DLIBRAW_FORCE_OPENMP"
      omp_libs="-lomp"
    else
      echo "WARNUNG: libomp fehlt (brew install libomp) — Fotos mit 6x6-Farbmuster dekodieren dann einkernig"
    fi
    ;;
  Linux)
    gomp="$(${CC:-gcc} -print-file-name=libgomp.a)"
    if [ -f "$gomp" ]; then
      cp "$gomp" "$ziel/lib/"
      openmp_schalter="--enable-openmp"
    else
      echo "WARNUNG: libgomp.a fehlt — Fotos mit 6x6-Farbmuster dekodieren dann einkernig"
    fi
    ;;
esac

echo "== LibRaw $LIBRAW_VERSION =="
curl -fsSL -o "$arbeit/libraw.tar.gz" "https://www.libraw.org/data/LibRaw-$LIBRAW_VERSION.tar.gz"
pruefen "$arbeit/libraw.tar.gz" "$LIBRAW_SHA256"
tar xzf "$arbeit/libraw.tar.gz" -C "$arbeit"
cd "$arbeit/LibRaw-$LIBRAW_VERSION"
# zlib (Deflate-DNG) liefert das Betriebssystem — die gibt es ueberall.
./configure --prefix="$ziel" --libdir="$ziel/lib" \
  --disable-shared --enable-static --with-pic \
  $openmp_schalter --disable-examples --disable-jasper --disable-lcms \
  --enable-zlib --enable-jpeg \
  CPPFLAGS="-I$ziel/include $omp_flags" LDFLAGS="-L$ziel/lib" LIBS="$omp_libs" \
  >/dev/null
# Nicht still ohne bauen: fehlt zlib.h oder jpeglib.h, schaltet configure die
# Unterstuetzung kommentarlos ab — und Deflate-/verlustbehaftete DNGs gingen
# dann nicht mehr.
for schalter in USE_ZLIB USE_JPEG; do
  grep -q -- "-D$schalter" Makefile || { echo "FEHLER: LibRaw ohne $schalter konfiguriert (Entwicklungsdateien fehlen?)"; exit 1; }
done
if [ "$openmp_schalter" = "--enable-openmp" ] || [ -n "$omp_flags" ]; then
  grep -q -- "openmp" Makefile || { echo "FEHLER: LibRaw ohne OpenMP konfiguriert"; exit 1; }
fi
make -j "$kerne" >/dev/null
make install >/dev/null

echo "== fertig =="
ls -l "$ziel/lib/"*.a
