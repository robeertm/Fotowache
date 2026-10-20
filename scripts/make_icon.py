#!/usr/bin/env python3
"""Erzeugt das Programmsymbol als PNG — ohne Fremdbibliotheken.

Bewusst reines Python: der Bau des Programmpakets soll auf einem frischen Rechner
laufen, ohne dass vorher noch eine Bildbibliothek nachinstalliert werden muss.

Motiv: eine Blende (wie im Objektiv) auf dunklem, abgerundetem Grund. Die sechs
Blendenlamellen sind in den Farben eines Farbkreises gehalten — das Programm
entwickelt Farbe, das darf man dem Symbol ansehen.
"""
import math
import struct
import sys
import zlib

SIZE = 1024


def rounded_alpha(x, y, size, radius):
    """Weiche Kante fuer das abgerundete Quadrat (Stil heutiger Programmsymbole)."""
    cx = min(max(x, radius), size - radius)
    cy = min(max(y, radius), size - radius)
    d = math.hypot(x - cx, y - cy)
    return max(0.0, min(1.0, radius - d + 0.5))


def blade_color(i):
    # Farbkreis in sechs Schritten, leicht entsaettigt — knallige Primaerfarben
    # wirken auf einem kleinen Symbol schnell billig.
    hue = i / 6.0
    r, g, b = hsv_to_rgb(hue, 0.62, 0.98)
    return int(r * 255), int(g * 255), int(b * 255)


def hsv_to_rgb(h, s, v):
    i = int(h * 6) % 6
    f = h * 6 - int(h * 6)
    p, q, t = v * (1 - s), v * (1 - s * f), v * (1 - s * (1 - f))
    return [(v, t, p), (q, v, p), (p, v, t), (p, q, v), (t, p, v), (v, p, q)][i]


def build():
    size = SIZE
    px = bytearray(size * size * 4)
    cx = cy = size / 2
    r_out = size * 0.34
    r_in = size * 0.085

    for y in range(size):
        for x in range(size):
            o = (y * size + x) * 4
            a = rounded_alpha(x, y, size, size * 0.22)
            if a <= 0:
                continue
            # Grund: dunkler Verlauf von oben links nach unten rechts
            t = (x + y) / (2 * size)
            base = (22 + 14 * t, 22 + 16 * t, 30 + 18 * t)

            dx, dy = x - cx, y - cy
            dist = math.hypot(dx, dy)
            col = base
            if dist < r_out:
                ang = (math.atan2(dy, dx) + math.pi) / (2 * math.pi)
                blade = int(ang * 6) % 6
                br, bg, bb = blade_color(blade)
                # Zur Mitte hin abdunkeln: gibt der Blende Tiefe.
                k = 0.35 + 0.65 * min(1.0, (dist - r_in) / (r_out - r_in))
                k = max(0.0, k)
                col = (br * k, bg * k, bb * k)
                # Weiche Aussenkante
                edge = min(1.0, (r_out - dist) / 6.0)
                col = tuple(base[i] + (col[i] - base[i]) * edge for i in range(3))
            if dist < r_in:
                # Offene Mitte
                col = (10, 10, 13)

            px[o] = int(col[0])
            px[o + 1] = int(col[1])
            px[o + 2] = int(col[2])
            px[o + 3] = int(255 * a)
    return bytes(px)


def write_png(path, rgba, size):
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # Filter „None"
        raw += rgba[y * stride : (y + 1) * stride]

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    if hasattr(path, "write"):  # Puffer (fuer die .ico)
        path.write(png)
        return
    with open(path, "wb") as f:
        f.write(png)


def write_ico(path, groessen=(16, 24, 32, 48, 64, 128, 256)):
    """.ico-Symbol: mehrere Groessen in einer Datei, jede als PNG eingebettet
    (das versteht das Zielsystem seit 2007). Jede Groesse wird eigens gerechnet
    statt herunterskaliert — so bleiben die Lamellen auch bei 16 px erkennbar."""
    import io
    bilder = []
    for g in groessen:
        globals()["SIZE"] = g
        tmp = io.BytesIO()
        write_png(tmp, build(), g)
        bilder.append((g, tmp.getvalue()))
    kopf = struct.pack("<HHH", 0, 1, len(bilder))
    versatz = 6 + 16 * len(bilder)
    eintraege = b""
    daten = b""
    for g, png in bilder:
        seite = 0 if g >= 256 else g
        eintraege += struct.pack("<BBBBHHII", seite, seite, 0, 0, 1, 32, len(png), versatz + len(daten))
        daten += png
    with open(path, "wb") as f:
        f.write(kopf + eintraege + daten)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
    if out.lower().endswith(".ico"):
        write_ico(out)
        print(f"geschrieben: {out}")
        sys.exit(0)
    # Zweites Argument: Kantenlaenge. Das .icns-Symbol braucht 1024 (daraus
    # rechnet das Paketskript alle kleineren Groessen), fuer .desktop-Starter
    # liegt lieber ein fertiges 256er bei.
    groesse = int(sys.argv[2]) if len(sys.argv) > 2 else SIZE
    globals()["SIZE"] = groesse
    write_png(out, build(), groesse)
    print(f"geschrieben: {out} ({groesse}×{groesse})")
