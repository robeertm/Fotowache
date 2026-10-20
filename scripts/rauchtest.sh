#!/usr/bin/env bash
# Rauchtest fuer ein fertiges Paket: Version abfragen, dann zwei echte RAW-Dateien
# (gemeinfreie Muster von raw.pixls.us, CC0) entwickeln — eine Bayer-DNG und
# eine RAF mit 6x6-Farbmuster. Prueft, dass JPEGs mit plausibler Groesse entstehen.
#
# Aufruf: scripts/rauchtest.sh <pfad/zu/fotowache>
# Ohne Grafikkarte (Bau-Rechner ohne GPU, auch ohne Software-Renderer) wird nur die
# Version geprueft und das deutlich gemeldet — nie still „gruen".
set -euo pipefail

prog="${1:?Aufruf: rauchtest.sh <fotowache>}"
"$prog" --version

arbeit="$(mktemp -d)"
trap 'rm -rf "$arbeit"' EXIT
mkdir -p "$arbeit/raw" "$arbeit/aus"

laden() { # name summe url
  curl -fsSL --retry 3 -o "$arbeit/raw/$1" "$3"
  if command -v sha256sum >/dev/null; then
    echo "$2  $arbeit/raw/$1" | sha256sum -c - >/dev/null
  else
    echo "$2  $arbeit/raw/$1" | shasum -a 256 -c - >/dev/null
  fi
}
laden probe_bayer.dng 99bcffd118630af5cf5712eeac6832045420311fe258094e3489b94c8d51236a \
  "https://raw.pixls.us/getfile.php/7278/nice/Sigma%20-%20fp%20-%2012bit%20%2816%3A9%29.DNG"  # Kennung: Download-Adresse
laden probe_6x6.raf 871836938ffa695f4c41765ddb107c2703ce755f7f3ced7e10f9e04746f1734e \
  "https://raw.pixls.us/getfile.php/3918/nice/Fujifilm%20-%20X-T4%20-%2014bit%2014bit%20compressed%20%283%3A2%29.RAF"  # Kennung: Download-Adresse

set +e
ausgabe="$("$prog" --stapel "$arbeit/raw" --ziel "$arbeit/aus" --breite 1200 2>&1)"
rc=$?
set -e
echo "$ausgabe"
# Exit 3 = keine Grafikkarte (siehe run_batch_cli).
if [ $rc -eq 3 ]; then
  echo "RAUCHTEST: keine Grafikkarte auf diesem Rechner — nur die Version wurde geprueft."
  exit 0
fi
[ $rc -eq 0 ] || { echo "RAUCHTEST FEHLGESCHLAGEN (Exit $rc)"; exit 1; }

anzahl=0
for f in "$arbeit/aus/"*.jpg; do
  [ -f "$f" ] || continue
  groesse=$(wc -c <"$f")
  echo "  $(basename "$f"): $groesse Byte"
  [ "$groesse" -gt 50000 ] || { echo "RAUCHTEST FEHLGESCHLAGEN: $f zu klein"; exit 1; }
  anzahl=$((anzahl + 1))
done
[ "$anzahl" -eq 2 ] || { echo "RAUCHTEST FEHLGESCHLAGEN: $anzahl statt 2 Bilder"; exit 1; }
echo "RAUCHTEST OK: 2 Bilder entwickelt"
