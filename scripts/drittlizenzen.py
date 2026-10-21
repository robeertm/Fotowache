#!/usr/bin/env python3
"""Schreibt THIRD-PARTY-NOTICES.md: alle fremden Bestandteile mit Lizenz.

Zwei Teile:
1. die C/C++-Bibliotheken und Daten, die in den Paketen stecken (LibRaw,
   libjpeg-turbo, OpenMP-Laufzeit, lensfun, DNG-SDK-Grundkurve) — von Hand
   gepflegt, die vollen Texte liegen in licenses/;
2. jede Rust-Bibliothek aus Cargo.lock mit ihrer Lizenzangabe und den
   Lizenztexten aus ihrem Quellpaket (gleiche Texte nur einmal).

Aufruf (im Workspace-Ordner): python3 scripts/drittlizenzen.py > THIRD-PARTY-NOTICES.md
"""
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

KOPF = """# Third-party notices

Fotowache is built on the work of others. This file lists every third-party
component that is part of the Fotowache packages, with its licence. Fotowache's own
licence is in [LICENSE](LICENSE); it does not limit the rights these licences
grant for their components.

## Libraries and data in the packages

**LibRaw 0.22.2** — RAW decoding. Copyright © 2008-2026 LibRaw LLC. Used
unmodified under the Common Development and Distribution License 1.0
([licenses/LibRaw-LICENSE.CDDL.txt](licenses/LibRaw-LICENSE.CDDL.txt),
[licenses/LibRaw-COPYRIGHT.txt](licenses/LibRaw-COPYRIGHT.txt)). Source code:
<https://www.libraw.org/download> — the packages are built from the release
tarball `LibRaw-0.22.2.tar.gz` (SHA-256 in `scripts/libraw-bauen.sh`; on
Windows the vcpkg port `libraw` 0.22.2).

**libjpeg-turbo 3.1.4.1** — lossy DNG decoding. This software is based in part
on the work of the Independent JPEG Group. IJG License, Modified (3-clause) BSD
License and zlib License
([licenses/libjpeg-turbo-LICENSE.md](licenses/libjpeg-turbo-LICENSE.md)).

**OpenMP runtime** — multi-core demosaicing. macOS: LLVM `libomp`, Apache
License 2.0 with LLVM Exceptions
([licenses/LLVM-OpenMP-LICENSE.txt](licenses/LLVM-OpenMP-LICENSE.txt)).
Linux: GCC `libgomp`, GNU GPL 3 with the GCC Runtime Library Exception
(<https://www.gnu.org/licenses/gcc-exception-3.1.html>). Windows: Microsoft
`vcomp140.dll`, distributed as a Visual C++ redistributable file under the
Microsoft Visual Studio licence terms.

**zlib** — Deflate-compressed DNGs; the copy that comes with the operating
system is used (Windows: built in through vcpkg, zlib License).

**lensfun lens database** — lens distortion, chromatic aberration and
vignetting profiles. Copyright © the lensfun contributors,
<https://lensfun.github.io/>. Licensed under Creative Commons
Attribution-ShareAlike 3.0
([licenses/lensfun-LICENSE.txt](licenses/lensfun-LICENSE.txt)). Fotowache ships
a compressed copy of the database files (`crates/pipeline/daten/lensfun.xml.z`,
made by `scripts/lensfun_aktualisieren.py`); that copy is distributed under the
same licence.

**DNG SDK** — the default base tone curve in `crates/pipeline/src/grundkurve.rs`
comes from the DNG SDK. This product includes DNG technology under license by
Adobe Systems Incorporated
([licenses/DNG-SDK-LICENSE.txt](licenses/DNG-SDK-LICENSE.txt)).

Camera and lens profiles of other software are NOT included. Fotowache only reads
profiles that other software has installed on the user's computer.

## Optional downloads (local AI, image generator)

These components are NOT part of the packages. They are downloaded only when the
user clicks “Install local AI” or “Install image generator”, directly from the
original publishers, each pinned to a fixed version and verified by SHA-256.
They keep their own licences:

- **llama.cpp** (inference runtime for the vision model), MIT License,
  Copyright © the ggml authors — <https://github.com/ggml-org/llama.cpp>
- **Huihui-Qwen3.5-4B-abliterated** (vision-language model, GGUF build by
  mradermacher), Apache License 2.0, derived from Qwen3.5-4B by the Qwen team
- **LaMa** inpainting model (ONNX export by Carve), Apache License 2.0,
  Copyright © the LaMa authors — <https://github.com/advimman/lama>
- **stable-diffusion.cpp** (inference runtime for the image generator), MIT
  License, Copyright © leejet — <https://github.com/leejet/stable-diffusion.cpp>
- **FLUX.2 [klein] 4B** (image model, GGUF build by leejet, and its VAE),
  Apache License 2.0, Copyright © Black Forest Labs
- **Qwen3-4B** (text encoder of the image generator, GGUF build by unsloth),
  Apache License 2.0, Copyright © the Qwen team

## Rust libraries

The following crates are compiled into Fotowache. Each entry gives the licence
declared by the crate; the licence texts follow below.

"""


def main() -> int:
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1"], text=True))
    eigene = set(meta["workspace_members"])
    pakete = sorted((p for p in meta["packages"] if p["id"] not in eigene),
                    key=lambda p: (p["name"], p["version"]))
    texte = {}       # hash -> (Text, [Pakete])
    zeilen = []
    for p in pakete:
        lizenz = p.get("license") or "see licence file"
        zeilen.append(f"- `{p['name']}` {p['version']} — {lizenz}"
                      + (f" — <{p['repository']}>" if p.get("repository") else ""))
        ordner = Path(p["manifest_path"]).parent
        kandidaten = [f for f in sorted(ordner.iterdir()) if f.is_file() and f.name.upper().startswith(
            ("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT"))]
        # Die Schriften in egui bringen eigene Lizenztexte mit.
        fonts = ordner / "fonts"
        if fonts.is_dir():
            kandidaten += [f for f in sorted(fonts.iterdir()) if f.suffix.lower() == ".txt"]
        for f in kandidaten:
            try:
                text = f.read_text(encoding="utf-8", errors="replace").strip()
            except OSError:
                continue
            if not text:
                continue
            h = hashlib.sha256(text.encode()).hexdigest()
            texte.setdefault(h, (text, []))[1].append(f"{p['name']} {p['version']} ({f.name})")
    out = [KOPF, "\n".join(zeilen), "\n\n## Licence texts of the Rust libraries\n"]
    for text, wer in sorted(texte.values()):
        out.append("\n---\n\nUsed by: " + ", ".join(sorted(set(wer))) + "\n\n```text\n" + text + "\n```\n")

    # Crates ohne eigene Lizenzdatei: die Standardtexte der angegebenen
    # Lizenzen (SPDX) — bei MIT mit den Autoren als Rechteinhabern.
    ohne = [p for p in pakete if not any(
        f"{p['name']} {p['version']} (" in w for _, ws in texte.values() for w in ws)]
    spdx = Path(__file__).resolve().parent.parent / "licenses" / "spdx"
    if ohne:
        out.append("\n## Crates without a licence file of their own\n\n"
                   "These crates declare their licence only in their package metadata. "
                   "The standard texts of those licences follow; copyright belongs to the "
                   "crate authors named here.\n\n")
        for p in ohne:
            autoren = ", ".join(a.split(" <")[0] for a in p.get("authors") or []) or f"the {p['name']} authors"
            out.append(f"- `{p['name']}` {p['version']} — {p.get('license')} — Copyright (c) {autoren}\n")
        kennungen = sorted({t for p in ohne for t in re.findall(r"[A-Za-z0-9.+-]+", p.get("license") or "")
                            if t not in ("OR", "AND", "WITH")})
        for k in kennungen:
            datei = spdx / f"{k}.txt"
            if datei.exists():
                out.append(f"\n### {k}\n\n```text\n{datei.read_text(encoding='utf-8').strip()}\n```\n")
    sys.stdout.write("".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
