#!/usr/bin/env python3
"""Bundle the lensfun lens database into the app.

Downloads all XML files of the lensfun database (data/db) from the lensfun
GitHub repository, joins them and writes a zlib-compressed blob that the
pipeline crate embeds (crates/pipeline/daten/lensfun.xml.z).

The lensfun database is licensed CC BY-SA 3.0 (see daten/LENSFUN-LICENSE.txt).
Only the data is used; Fotowache implements the correction models itself.
"""
import json
import sys
import urllib.request
import zlib
from pathlib import Path

API = "https://api.github.com/repos/lensfun/lensfun/contents/data/db?ref=master"
RAW = "https://raw.githubusercontent.com/lensfun/lensfun/master/data/db/"
ZIEL = Path(__file__).resolve().parent.parent / "crates" / "pipeline" / "daten"


def holen(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read()


def main() -> int:
    liste = json.loads(holen(API))
    namen = sorted(e["name"] for e in liste if e["name"].endswith(".xml"))
    teile = []
    for n in namen:
        teile.append(f"<!-- datei: {n} -->\n".encode() + holen(RAW + n))
    roh = b"\n".join(teile)
    ZIEL.mkdir(parents=True, exist_ok=True)
    (ZIEL / "lensfun.xml.z").write_bytes(zlib.compress(roh, 9))
    print(f"{len(namen)} Dateien, {len(roh)/1e6:.1f} MB -> {len(zlib.compress(roh, 9))/1e6:.2f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
