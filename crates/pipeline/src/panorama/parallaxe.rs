//! Parallaxe nach Zhang & Liu, "Parallax-tolerant Image Stitching", CVPR 2014
//! (im Folgenden Z&L).
//!
//! ## Idee der Arbeit
//! Bei Parallaxe (Kamera nicht um ihr Projektionszentrum gedreht) gibt es keine
//! Abbildung, die die ganze Ueberlappung deckt. Noetig ist das auch nicht: es
//! genuegt, die Bilder in EINEM oertlichen Bereich so auszurichten, dass dort
//! eine Naht unsichtbar laufen kann ("local stitching"). Dazu:
//! 1. zufaellige Suche nach einer Homographie, die eine raeumlich kompakte
//!    Gruppe von Merkmalspaaren "locker" ausrichtet (Abschnitt 3.3, Schritte
//!    1–3): Startmerkmal, naechste Nachbarn eines nach dem anderen dazu, bis die
//!    Gruppe nicht mehr zu einer Homographie passt; Strafzaehler je Merkmal;
//! 2. Bewertung jeder Homographie durch die Kosten einer plausiblen Naht
//!    (Abschnitt 3.2): Graphenschnitt auf der Differenz geglaetteter
//!    Kantenkarten, Kanten gewichtet mit dem Vertrauen f_c (Gl. 1–2); verworfen
//!    wird, was zu stark perspektivisch verzerrt (Gl. 3, Schwelle 0,01);
//! 3. inhaltserhaltende Netzverformung (Abschnitt 3.3.1, Gl. 4–8), die die
//!    gewaehlten Merkmale genau zur Deckung bringt; danach wie immer Naht und
//!    Mehrband-Mischung.
//!
//! ## Uebertragung auf viele Bilder (die Arbeit behandelt zwei)
//! - Vorverformung (Ī in Z&L) ist die Geometrie aus Teil A (Brown & Lowe) auf
//!   der Projektionsflaeche — sie ist die global beste Abbildung. Die gefundene
//!   Homographie dient nur der Auswahl der Merkmalsgruppe und der Bewertung;
//!   verformt wird mit dem Netz. Ausserhalb der Ueberlappungen haelt Eg (Gl. 5)
//!   das Netz auf der Teil-A-Geometrie fest — so bleibt die Ausrichtung zu den
//!   uebrigen Nachbarn erhalten.
//! - Z&L verformen ein Bild auf ein festes Bezugsbild. Hier bleibt das Bild in
//!   der Mitte des staerksten Spannbaums unverformt (wie das Bezugsbild); jedes
//!   andere Bild wird auf seine Nachbarn ausgerichtet, die naeher an dieser
//!   Mitte liegen (gleicher Abstand: der mit kleinerem Index ist Bezug). Die
//!   Bilder werden von innen nach aussen bearbeitet, die Ziele P̃ sind die
//!   Stellen im SCHON VERFORMTEN Bezugsbild.
//! - Nur Paare mit messbarem Restversatz nach Teil A werden behandelt: Median
//!   des Rueckprojektionsfehlers der Punktpaare > 1 px (Eingabepixel) — das
//!   Ziel der Messung "Median < 1 px".
//!
//! ## Werte
//! Aus der Arbeit: δ = 0,01 (Gl. 2), Screening-Schwelle 0,01 (Gl. 3), α = 0,01,
//! β = 0,001 (Gl. 8). Aus Liu u. a. 2009 ("Content-preserving warps for 3D video
//! stabilization", auf das Z&L fuer Es und w_s verweisen): Gitter 64 Zellen
//! entlang der laengeren Seite (dort 64 × 36), w_s = L2-Norm der Farbvarianz
//! der Zelle + 0,5, Es ueber alle acht Dreiecke jeder Ecke, Merkmale per
//! umgekehrter bilinearer Interpolation (Heckbert 1989) im Netz verankert.
//! Nicht in den Arbeiten angegeben und deshalb hier festgelegt (siehe die
//! Konstanten unten): Kantenschwellen, Glaettung, Gauss-Breite in f_c,
//! Passschwelle der Gruppe, Abbruch der Suche. Z&L Gl. 5 setzt τ_i = 1, wenn
//! ein Merkmal in der Naehe liegt — der Text davor sagt aber, gerade die
//! Bereiche OHNE Merkmale sollen bei der Vorverformung bleiben; umgesetzt ist
//! die erklaerte Absicht (τ_i = 1 ohne Merkmal in den vier angrenzenden Zellen).

use super::geometrie::{dlt, P2};
use super::mathe::Zufall;
use super::naht::Graph;
use super::projektion::{Kamera, Leinwand};
use rayon::prelude::*;

/// Z&L Gl. 2.
const DELTA: f64 = 0.01;
/// Z&L Gl. 3.
const SCREENING: f64 = 0.01;
/// Z&L Gl. 8.
const ALPHA: f64 = 0.01;
const BETA: f64 = 0.001;
/// Liu 2009: Zellen entlang der laengeren Seite.
const ZELLEN: usize = 64;
/// Liu 2009: Zuschlag zur Auffaelligkeit.
const W_S_EPSILON: f64 = 0.5;
/// Restversatz (Eingabepixel), ab dem ein Paar behandelt wird.
pub const SCHWELLE_PX: f64 = 1.0;
/// Passschwelle der Merkmalsgruppe (Z&L: "slightly large"): die Toleranz der
/// Zuordnung nach Brown & Lowe (RANSAC, 3 px auf der Merkmalskopie, ~12 px
/// voller Aufloesung), auf die Leinwand umgerechnet — groesser als die
/// Genauigkeit der nachgefuehrten Punkte, also "locker" im Sinne der Arbeit.
/// Gemessen gegen 2, 4 und 8 Eingabepixel (13 Reihen): kleinster Nahtversatz
/// in den meisten Reihen (z. B. 2081–2085 an der Naht 0,51 gegen 0,92/0,99/
/// 0,51 px), Liniengeradheit gleichauf (95 % der Segmente aendern den Winkel
/// im Median der Reihen um hoechstens 0,33 Grad; 2 px: 0,31, 4 px: 0,25).
const PASS_MERKMALSPIXEL: f64 = 3.0;
/// Bewertungsleinwand (Z&L: "down-sample"): Ueberlappung hoechstens so viele
/// Pixel.
const BEWERTUNG_PIXEL: f64 = 160_000.0;

/// Verformung eines Bildes auf der Leinwand: regelmaessiges Netz ueber dem
/// Teil-A-Bild (Vorverformung) und die verformten Ecken.
#[derive(Clone, Debug)]
pub struct Netz {
    pub x0: f64,
    pub y0: f64,
    pub zelle: f64,
    pub nx: usize,
    pub ny: usize,
    /// Verformte Ecken (Leinwandpixel), (nx + 1) * (ny + 1), zeilenweise.
    pub ecken: Vec<[f64; 2]>,
}

impl Netz {
    fn ecke(&self, i: usize, j: usize) -> [f64; 2] {
        self.ecken[j * (self.nx + 1) + i]
    }

    /// Teil-A-Leinwandstelle -> verformte Stelle (bilinear in der Zelle).
    pub fn vor(&self, x: f64, y: f64) -> (f64, f64) {
        let gx = ((x - self.x0) / self.zelle).clamp(0.0, self.nx as f64 - 1e-9);
        let gy = ((y - self.y0) / self.zelle).clamp(0.0, self.ny as f64 - 1e-9);
        let (i, j) = (gx as usize, gy as usize);
        let (u, v) = (gx - i as f64, gy - j as f64);
        // Ausserhalb des Netzes: Verschiebung der naechsten Randstelle.
        let (rx, ry) = (self.x0 + gx * self.zelle, self.y0 + gy * self.zelle);
        let (a, b, c, d) = (self.ecke(i, j), self.ecke(i + 1, j), self.ecke(i, j + 1), self.ecke(i + 1, j + 1));
        let px = (1.0 - u) * (1.0 - v) * a[0] + u * (1.0 - v) * b[0] + (1.0 - u) * v * c[0] + u * v * d[0];
        let py = (1.0 - u) * (1.0 - v) * a[1] + u * (1.0 - v) * b[1] + (1.0 - u) * v * c[1] + u * v * d[1];
        (px + (x - rx), py + (y - ry))
    }

    /// Verformte Stelle -> Teil-A-Leinwandstelle: Zelle suchen, deren
    /// verformtes Viereck die Stelle enthaelt, und dort die bilineare
    /// Interpolation umkehren (Heckbert 1989). Ausserhalb: Verschiebung der
    /// naechsten Randzelle.
    pub fn zurueck(&self, x: f64, y: f64) -> (f64, f64) {
        let gx = ((x - self.x0) / self.zelle).floor() as isize;
        let gy = ((y - self.y0) / self.zelle).floor() as isize;
        let mut beste: Option<(f64, f64, f64)> = None; // (Abstand, x, y)
        for r in 0..=3isize {
            for dj in -r..=r {
                for di in -r..=r {
                    if di.abs() != r && dj.abs() != r {
                        continue;
                    }
                    let (i, j) = (gx + di, gy + dj);
                    if i < 0 || j < 0 || i >= self.nx as isize || j >= self.ny as isize {
                        continue;
                    }
                    let (i, j) = (i as usize, j as usize);
                    let (u, v) = umgekehrt_bilinear(self.ecke(i, j), self.ecke(i + 1, j), self.ecke(i, j + 1), self.ecke(i + 1, j + 1), [x, y]);
                    let aussen = (-u).max(u - 1.0).max(-v).max(v - 1.0).max(0.0);
                    let q = (self.x0 + (i as f64 + u) * self.zelle, self.y0 + (j as f64 + v) * self.zelle);
                    if aussen <= 1e-9 {
                        return q;
                    }
                    if beste.is_none_or(|b| aussen < b.0) {
                        beste = Some((aussen, q.0, q.1));
                    }
                }
            }
            if r >= 1 && beste.is_some_and(|b| b.0 < 0.5) {
                break;
            }
        }
        match beste {
            Some((_, qx, qy)) => (qx, qy),
            None => {
                // Weit ausserhalb: Verschiebung der naechsten Ecke umkehren.
                let i = gx.clamp(0, self.nx as isize) as usize;
                let j = gy.clamp(0, self.ny as isize) as usize;
                let e = self.ecke(i, j);
                let (rx, ry) = (self.x0 + i as f64 * self.zelle, self.y0 + j as f64 * self.zelle);
                (x - (e[0] - rx), y - (e[1] - ry))
            }
        }
    }

    /// Groesste Verschiebung einer Ecke (Leinwandpixel).
    pub fn groesste(&self) -> f64 {
        let mut m = 0.0f64;
        for j in 0..=self.ny {
            for i in 0..=self.nx {
                let e = self.ecke(i, j);
                m = m.max((e[0] - (self.x0 + i as f64 * self.zelle)).hypot(e[1] - (self.y0 + j as f64 * self.zelle)));
            }
        }
        m
    }

    /// Groesste Drehung einer Zellkante gegen die Vorverformung (Grad) — ein
    /// Mass dafuer, wie stark Geraden gebogen werden koennten.
    pub fn groesste_neigung_grad(&self) -> f64 {
        let mut m = 0.0f64;
        for j in 0..=self.ny {
            for i in 0..=self.nx {
                let a = self.ecke(i, j);
                if i < self.nx {
                    let b = self.ecke(i + 1, j);
                    m = m.max((b[1] - a[1]).atan2(b[0] - a[0]).abs());
                }
                if j < self.ny {
                    let b = self.ecke(i, j + 1);
                    m = m.max((b[0] - a[0]).atan2(b[1] - a[1]).abs());
                }
            }
        }
        m.to_degrees()
    }
}

/// Umkehrung der bilinearen Interpolation im Viereck a (0,0), b (1,0),
/// c (0,1), d (1,1) (Heckbert 1989): (u, v) mit P = bilinear(u, v).
fn umgekehrt_bilinear(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], p: [f64; 2]) -> (f64, f64) {
    // P = A + u E + v F + u v G
    let e = [b[0] - a[0], b[1] - a[1]];
    let f = [c[0] - a[0], c[1] - a[1]];
    let g = [a[0] - b[0] - c[0] + d[0], a[1] - b[1] - c[1] + d[1]];
    let h = [p[0] - a[0], p[1] - a[1]];
    let kreuz = |x: [f64; 2], y: [f64; 2]| x[0] * y[1] - x[1] * y[0];
    let k2 = kreuz(g, f);
    let k1 = kreuz(e, f) + kreuz(h, g);
    let k0 = kreuz(h, e);
    let v = if k2.abs() < 1e-12 * (k1.abs() + 1e-300) || k2 == 0.0 {
        if k1.abs() < 1e-300 {
            return (0.0, 0.0);
        }
        -k0 / k1
    } else {
        let disk = (k1 * k1 - 4.0 * k0 * k2).max(0.0).sqrt();
        let v1 = (-k1 + disk) / (2.0 * k2);
        let v2 = (-k1 - disk) / (2.0 * k2);
        if (v1 - 0.5).abs() <= (v2 - 0.5).abs() {
            v1
        } else {
            v2
        }
    };
    let nenner_x = e[0] + g[0] * v;
    let nenner_y = e[1] + g[1] * v;
    let u = if nenner_x.abs() > nenner_y.abs() { (h[0] - f[0] * v) / nenner_x } else { (h[1] - f[1] * v) / nenner_y };
    (u, v)
}

/// Ein Bild aus Sicht der Parallaxe: Kamera (Teil A), Merkmalskopie (fuer
/// Kantenkarten und Auffaelligkeit), Rechteck auf der Leinwand.
pub struct Bild<'a> {
    pub kamera: Kamera,
    pub rgb: &'a [f32],
    pub w: usize,
    pub h: usize,
    /// Massstab der Merkmalskopie zur vollen Aufloesung.
    pub s: f64,
    pub rechteck: (usize, usize, usize, usize),
}

/// Ein Merkmalspaar auf der Leinwand (Teil-A-Stellen in beiden Bildern).
#[derive(Clone, Copy)]
pub struct Punkt {
    pub a: P2,
    pub b: P2,
}

/// Ein Bildpaar fuer die Parallaxe (Indizes der verwendeten Bilder).
pub struct Paar {
    pub i: usize,
    pub j: usize,
    pub punkte: Vec<Punkt>,
}

/// Ergebnis: Netz je Bild (None = unverformt) und Zahlen fuer den Bericht.
pub struct Ausgleich {
    pub netze: Vec<Option<Netz>>,
    /// Median / 90 % des Restversatzes der behandelten Paare vorher (alle
    /// Punktpaare) und nachher (die ausgerichteten Merkmalsgruppen), Leinwandpx.
    pub vorher: (f64, f64),
    pub nachher: (f64, f64),
    pub paare: usize,
}

fn quantil(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Abbildung einer Teil-A-Leinwandstelle eines Bildes auf seine endgueltige
/// Stelle (mit Netz, falls verformt).
fn endgueltig(netz: &Option<Netz>, p: P2) -> P2 {
    match netz {
        Some(n) => n.vor(p.0, p.1),
        None => p,
    }
}

/// Graustufen eines Bildes auf einer Bewertungsleinwand (Massstab `m` zur
/// vollen Leinwand, Ausschnitt ab (ox, oy), Groesse bw x bh); `abbildung`:
/// volle Leinwandstelle -> Teil-A-Leinwandstelle (Umkehrung eines Netzes).
#[allow(clippy::too_many_arguments)]
fn grau_auf_leinwand(
    l: &Leinwand,
    b: &Bild,
    m: f64,
    ox: f64,
    oy: f64,
    bw: usize,
    bh: usize,
    abbildung: &(dyn Fn(f64, f64) -> (f64, f64) + Sync),
) -> (Vec<f32>, Vec<bool>, Vec<[f32; 3]>) {
    let mut g = vec![0.0f32; bw * bh];
    let mut ok = vec![false; bw * bh];
    let mut farbe = vec![[0.0f32; 3]; bw * bh];
    g.par_chunks_mut(bw).zip(ok.par_chunks_mut(bw)).zip(farbe.par_chunks_mut(bw)).enumerate().for_each(|(y, ((zg, zo), zf))| {
        for x in 0..bw {
            let (fx, fy) = (ox + (x as f64 + 0.5) / m, oy + (y as f64 + 0.5) / m);
            let (ax, ay) = abbildung(fx, fy);
            let Some((sx, sy)) = super::naht::quelle_f(l, &b.kamera, ax, ay) else { continue };
            if sx < 0.5 || sy < 0.5 || sx > b.kamera.w - 0.5 || sy > b.kamera.h - 0.5 {
                continue;
            }
            let p = super::naht::abtasten(b.rgb, b.w, b.h, (sx * b.s) as f32, (sy * b.s) as f32);
            let k = p.map(|v| 255.0 * v.max(0.0).powf(1.0 / 2.2));
            zg[x] = 0.3 * k[0] + 0.55 * k[1] + 0.15 * k[2];
            zo[x] = true;
            zf[x] = k;
        }
    });
    (g, ok, farbe)
}

fn gauss_glatt(d: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp() as f32).collect();
    let summe: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / summe).collect();
    let mut t = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (o, kv) in k.iter().enumerate() {
                let xx = (x as isize + o as isize - r).clamp(0, w as isize - 1) as usize;
                s += d[y * w + xx] * kv;
            }
            t[y * w + x] = s;
        }
    }
    let mut a = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (o, kv) in k.iter().enumerate() {
                let yy = (y as isize + o as isize - r).clamp(0, h as isize - 1) as usize;
                s += t[yy * w + x] * kv;
            }
            a[y * w + x] = s;
        }
    }
    a
}

/// Kantenkarte nach Canny 1986 (Z&L Abschnitt 3.2): Gauss-Glaettung σ = 1,
/// Sobel-Gradient, Unterdrueckung der Nicht-Maxima, Hysterese. Die Schwellen
/// nennt Z&L nicht: obere = oberes Dezil der Gradientenbetraege im gueltigen
/// Bereich, untere = 0,4 der oberen (Canny empfiehlt ein Verhaeltnis von 2:1
/// bis 3:1). Ergebnis 0/1.
fn canny(g: &[f32], ok: &[bool], w: usize, h: usize) -> Vec<f32> {
    let gl = gauss_glatt(g, w, h, 1.0);
    let mut mag = vec![0.0f32; w * h];
    let mut rich = vec![0u8; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let p = |dx: isize, dy: isize| gl[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            let gx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1)) - (p(-1, -1) + 2.0 * p(-1, 0) + p(-1, 1));
            let gy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)) - (p(-1, -1) + 2.0 * p(0, -1) + p(1, -1));
            let i = y * w + x;
            if !(ok[i] && ok[i - 1] && ok[i + 1] && ok[i - w] && ok[i + w]) {
                continue;
            }
            mag[i] = gx.hypot(gy);
            let a = gy.atan2(gx).to_degrees().rem_euclid(180.0);
            rich[i] = if !(22.5..157.5).contains(&a) {
                0
            } else if a < 67.5 {
                1
            } else if a < 112.5 {
                2
            } else {
                3
            };
        }
    }
    let mut werte: Vec<f32> = mag.iter().copied().filter(|v| *v > 0.0).collect();
    if werte.is_empty() {
        return vec![0.0; w * h];
    }
    werte.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let hoch = werte[(werte.len() as f64 * 0.9) as usize];
    let tief = 0.4 * hoch;
    let mut nms = vec![0.0f32; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            let m = mag[i];
            if m < tief {
                continue;
            }
            let (a, b) = match rich[i] {
                0 => (mag[i - 1], mag[i + 1]),
                1 => (mag[i - w - 1], mag[i + w + 1]),
                2 => (mag[i - w], mag[i + w]),
                _ => (mag[i - w + 1], mag[i + w - 1]),
            };
            if m >= a && m >= b {
                nms[i] = m;
            }
        }
    }
    let mut kante = vec![0.0f32; w * h];
    let mut stapel: Vec<usize> = (0..w * h).filter(|&i| nms[i] >= hoch).collect();
    for &i in &stapel {
        kante[i] = 1.0;
    }
    while let Some(i) = stapel.pop() {
        let (x, y) = (i % w, i / w);
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if kante[j] == 0.0 && nms[j] >= tief {
                    kante[j] = 1.0;
                    stapel.push(j);
                }
            }
        }
    }
    kante
}

fn h_abbilden(h: &[[f64; 3]; 3], p: P2) -> P2 {
    super::geometrie::abbilden(h, p)
}

/// Z&L Gl. 3: Abweichung der Homographie von der bestpassenden Aehnlichkeit an
/// den vier Ecken, summiert und durch die Bildgroesse (Diagonale) geteilt.
fn perspektive(h: &[[f64; 3]; 3], ecken: &[P2; 4], diag: f64) -> f64 {
    let ziel: Vec<P2> = ecken.iter().map(|&c| h_abbilden(h, c)).collect();
    if ziel.iter().any(|p| !p.0.is_finite()) {
        return f64::INFINITY;
    }
    // Aehnlichkeit [a −b c; b a d] nach kleinsten Quadraten (geschlossen).
    let n = 4.0;
    let (mx, my) = (ecken.iter().map(|c| c.0).sum::<f64>() / n, ecken.iter().map(|c| c.1).sum::<f64>() / n);
    let (zx, zy) = (ziel.iter().map(|c| c.0).sum::<f64>() / n, ziel.iter().map(|c| c.1).sum::<f64>() / n);
    let (mut sa, mut sb, mut ss) = (0.0, 0.0, 0.0);
    for (c, z) in ecken.iter().zip(&ziel) {
        let (px, py, qx, qy) = (c.0 - mx, c.1 - my, z.0 - zx, z.1 - zy);
        sa += px * qx + py * qy;
        sb += px * qy - py * qx;
        ss += px * px + py * py;
    }
    let (a, b) = (sa / ss, sb / ss);
    let mut summe = 0.0;
    for (c, z) in ecken.iter().zip(&ziel) {
        let (px, py) = (c.0 - mx, c.1 - my);
        let s = (a * px - b * py + zx, b * px + a * py + zy);
        summe += (s.0 - z.0).hypot(s.1 - z.1);
    }
    summe / diag
}

/// Bewertung einer Homographie nach Z&L Abschnitt 3.2: Kosten der plausiblen
/// Naht auf der Bewertungsleinwand.
struct Bewertung<'a> {
    bw: usize,
    bh: usize,
    m: f64,
    ox: f64,
    oy: f64,
    /// Kantenkarten (geglaettet): bewegtes Bild (Teil A) und Bezug (endgueltig).
    kante_a: Vec<f32>,
    ok_a: Vec<bool>,
    kante_b: &'a [f32],
    ok_b: &'a [bool],
    /// σ der Gauss-Funktion in f_c (Bewertungspixel).
    sigma_g: f64,
}

impl Bewertung<'_> {
    /// Nahtkosten fuer Homographie `h` (volle Leinwand, Teil-A-Stelle des
    /// bewegten Bildes -> Zielstelle) und Merkmalsgruppe (Zielstellen).
    fn kosten(&self, h: &[[f64; 3]; 3], gruppe: &[P2]) -> f64 {
        let (bw, bh) = (self.bw, self.bh);
        let hi = match invertieren(h) {
            Some(v) => v,
            None => return f64::INFINITY,
        };
        // Kantenkarte des bewegten Bildes unter H (rueckwaerts abgetastet).
        let mut ka = vec![0.0f32; bw * bh];
        let mut oa = vec![false; bw * bh];
        for y in 0..bh {
            for x in 0..bw {
                let p = (self.ox + (x as f64 + 0.5) / self.m, self.oy + (y as f64 + 0.5) / self.m);
                let q = h_abbilden(&hi, p);
                let (qx, qy) = ((q.0 - self.ox) * self.m - 0.5, (q.1 - self.oy) * self.m - 0.5);
                if !(qx >= 0.0 && qy >= 0.0 && qx < (bw - 1) as f64 && qy < (bh - 1) as f64) {
                    continue;
                }
                let (x0, y0) = (qx as usize, qy as usize);
                let (fx, fy) = ((qx - x0 as f64) as f32, (qy - y0 as f64) as f32);
                let i = y0 * bw + x0;
                if !(self.ok_a[i] && self.ok_a[i + 1] && self.ok_a[i + bw] && self.ok_a[i + bw + 1]) {
                    continue;
                }
                let v = (self.kante_a[i] * (1.0 - fx) + self.kante_a[i + 1] * fx) * (1.0 - fy) + (self.kante_a[i + bw] * (1.0 - fx) + self.kante_a[i + bw + 1] * fx) * fy;
                ka[y * bw + x] = v;
                oa[y * bw + x] = true;
            }
        }
        // f_c (Gl. 2) aus den Merkmalen der Gruppe.
        let pts: Vec<(f64, f64)> = gruppe.iter().map(|p| ((p.0 - self.ox) * self.m, (p.1 - self.oy) * self.m)).collect();
        let s2 = 2.0 * self.sigma_g * self.sigma_g;
        let reichweite = 3.0 * self.sigma_g;
        let mut fc = vec![0.0f32; bw * bh];
        for y in 0..bh {
            for x in 0..bw {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let mut s = 0.0;
                for &(qx, qy) in &pts {
                    let (dx, dy) = (px - qx, py - qy);
                    if dx.abs() > reichweite || dy.abs() > reichweite {
                        continue;
                    }
                    s += (-(dx * dx + dy * dy) / s2).exp();
                }
                fc[y * bw + x] = (1.0 / (s + DELTA)) as f32;
            }
        }
        // Graphenschnitt (Gl. 1) auf der gemeinsamen Flaeche.
        let mut knoten = vec![u32::MAX; bw * bh];
        let mut n = 0usize;
        for i in 0..bw * bh {
            if oa[i] && self.ok_b[i] {
                knoten[i] = n as u32;
                n += 1;
            }
        }
        if n < 16 {
            return f64::INFINITY;
        }
        let ed: Vec<f32> = (0..bw * bh).map(|i| (ka[i] - self.kante_b[i]).abs()).collect();
        const SKALA: f64 = 1000.0;
        const GROSS: i64 = 1 << 40;
        let mut g = Graph::neu(n);
        for y in 0..bh {
            for x in 0..bw {
                let i = y * bw + x;
                let k = knoten[i];
                if k == u32::MAX {
                    continue;
                }
                for (nx, ny) in [(x as isize + 1, y as isize), (x as isize, y as isize + 1), (x as isize - 1, y as isize), (x as isize, y as isize - 1)] {
                    if nx < 0 || ny < 0 || nx >= bw as isize || ny >= bh as isize {
                        continue;
                    }
                    let j = ny as usize * bw + nx as usize;
                    if knoten[j] != u32::MAX {
                        if knoten[j] > k {
                            let c = (fc[i] * ed[i] + fc[j] * ed[j]) as f64;
                            let c = (c * SKALA) as i64 + 1;
                            g.kante(k as usize, knoten[j] as usize, c, c);
                        }
                    } else if oa[j] && !self.ok_b[j] {
                        g.endknoten(k as usize, GROSS, 0);
                    } else if self.ok_b[j] && !oa[j] {
                        g.endknoten(k as usize, 0, GROSS);
                    }
                }
            }
        }
        g.schneiden();
        g.fluss as f64 / SKALA
    }
}

#[allow(clippy::needless_range_loop)]
fn invertieren(h: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let d = super::mathe::det(h);
    if d.abs() < 1e-300 {
        return None;
    }
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let (a1, b1) = ((j + 1) % 3, (j + 2) % 3);
            let (c1, e1) = ((i + 1) % 3, (i + 2) % 3);
            r[i][j] = (h[a1][c1] * h[b1][e1] - h[a1][e1] * h[b1][c1]) / d;
        }
    }
    Some(r)
}

/// Zufaellige Suche nach Z&L Abschnitt 3.3, Schritte 1–3. Rueckgabe: Indizes
/// der gewaehlten Merkmalsgruppe (leer, wenn keine taugliche Homographie).
fn suchen(punkte: &[Punkt], bewertung: &Bewertung, ecken: &[P2; 4], diag: f64, pass: f64, saat: u64) -> Vec<usize> {
    let n = punkte.len();
    if n < 4 {
        return vec![];
    }
    let mut strafe = vec![0u32; n];
    let mut war_start = vec![false; n];
    let mut z = Zufall::neu(saat);
    let mut beste: Option<(f64, Vec<usize>)> = None;
    loop {
        let mittel = strafe.iter().map(|&s| s as f64).sum::<f64>() / n as f64;
        // Abbruch: die Arbeit setzt fort, "if the average penalty value is
        // low" — hier, bis jedes Merkmal im Mittel einmal gewaehlt war.
        if mittel >= 1.0 {
            break;
        }
        let gueltig: Vec<usize> = (0..n).filter(|&k| !war_start[k] && strafe[k] as f64 <= mittel).collect();
        if gueltig.is_empty() {
            break;
        }
        let start = gueltig[z.bis(gueltig.len())];
        war_start[start] = true;
        // Naechste Nachbarn (Teil-A-Stellen im bewegten Bild) nacheinander.
        let mut ordnung: Vec<usize> = (0..n).collect();
        let sp = punkte[start].a;
        ordnung.sort_by(|&x, &y| {
            let dx = (punkte[x].a.0 - sp.0).hypot(punkte[x].a.1 - sp.1);
            let dy = (punkte[y].a.0 - sp.0).hypot(punkte[y].a.1 - sp.1);
            dx.partial_cmp(&dy).unwrap()
        });
        let mut gruppe: Vec<usize> = Vec::new();
        let mut h_gruppe = None;
        for &k in &ordnung {
            gruppe.push(k);
            if gruppe.len() < 4 {
                continue;
            }
            let p: Vec<P2> = gruppe.iter().map(|&q| punkte[q].a).collect();
            let q: Vec<P2> = gruppe.iter().map(|&q| punkte[q].b).collect();
            let Some(h) = dlt(&p, &q) else {
                gruppe.pop();
                break;
            };
            let max = p.iter().zip(&q).map(|(a, b)| {
                let r = h_abbilden(&h, *a);
                (r.0 - b.0).hypot(r.1 - b.1)
            });
            if max.fold(0.0f64, f64::max) > pass {
                gruppe.pop();
                break;
            }
            h_gruppe = Some(h);
        }
        for &k in &gruppe {
            strafe[k] += 1;
        }
        let Some(h) = h_gruppe else { continue };
        let pv = perspektive(&h, ecken, diag);
        if pv > SCREENING {
            super::diagnose(&format!("    Parallaxe: Gruppe {} Merkmale verworfen (Perspektive {pv:.4})", gruppe.len()));
            continue;
        }
        let ziele: Vec<P2> = gruppe.iter().map(|&k| punkte[k].b).collect();
        let c = bewertung.kosten(&h, &ziele);
        super::diagnose(&format!("    Parallaxe: Gruppe {} Merkmale, Nahtkosten {c:.1}", gruppe.len()));
        if beste.as_ref().is_none_or(|b| c < b.0) {
            beste = Some((c, gruppe));
        }
    }
    beste.map(|b| b.1).unwrap_or_default()
}

/// Inhaltserhaltende Netzverformung (Z&L Abschnitt 3.3.1, Gl. 4–8) fuer ein
/// Bild: `ziele` = (Teil-A-Stelle, Zielstelle) der gewaehlten Merkmale.
/// Eine Zeile der Kleinste-Quadrate-Aufgabe: (Koeffizienten je Unbekannte,
/// rechte Seite, Gewicht).
type Zeile = (Vec<(usize, f64)>, f64, f64);

#[allow(clippy::needless_range_loop)]
fn netz_loesen(rechteck: (usize, usize, usize, usize), ziele: &[(P2, P2)], auffaellig: &dyn Fn(f64, f64, f64, f64) -> f64) -> Netz {
    let (x0, x1, y0, y1) = rechteck;
    let (bw, bh) = ((x1 - x0) as f64, (y1 - y0) as f64);
    let zelle = bw.max(bh) / ZELLEN as f64;
    let nx = (bw / zelle).ceil() as usize + 1;
    let ny = (bh / zelle).ceil() as usize + 1;
    let (ox, oy) = (x0 as f64 - 0.5 * zelle, y0 as f64 - 0.5 * zelle);
    let ecken_n = (nx + 1) * (ny + 1);
    let idx = |i: usize, j: usize| j * (nx + 1) + i;
    let v = |i: usize, j: usize| [ox + i as f64 * zelle, oy + j as f64 * zelle];
    // Zeilen der Kleinste-Quadrate-Aufgabe: Σ_k c_k * u_k (u = 2 * Ecken
    // Unbekannte, x und y verschraenkt) = rhs, Gewicht w.
    let mut zeilen: Vec<Zeile> = Vec::new();
    // Ep (Gl. 4): bilineare Koeffizienten im Vorverformungsnetz (regelmaessig).
    let mut hat_merkmal = vec![false; nx * ny];
    for &(p, z) in ziele {
        let gx = ((p.0 - ox) / zelle).clamp(0.0, nx as f64 - 1e-9);
        let gy = ((p.1 - oy) / zelle).clamp(0.0, ny as f64 - 1e-9);
        let (i, j) = (gx as usize, gy as usize);
        hat_merkmal[j * nx + i] = true;
        let (u, w) = (gx - i as f64, gy - j as f64);
        let koeff = [(idx(i, j), (1.0 - u) * (1.0 - w)), (idx(i + 1, j), u * (1.0 - w)), (idx(i, j + 1), (1.0 - u) * w), (idx(i + 1, j + 1), u * w)];
        for d in 0..2 {
            zeilen.push((koeff.iter().map(|&(k, c)| (2 * k + d, c)).collect(), if d == 0 { z.0 } else { z.1 }, 1.0));
        }
    }
    // Eg (Gl. 5): Ecken ohne Merkmal in den angrenzenden Zellen bleiben bei der
    // Vorverformung.
    for j in 0..=ny {
        for i in 0..=nx {
            let mut nah = false;
            for (ci, cj) in [(i.wrapping_sub(1), j.wrapping_sub(1)), (i, j.wrapping_sub(1)), (i.wrapping_sub(1), j), (i, j)] {
                if ci < nx && cj < ny && hat_merkmal[cj * nx + ci] {
                    nah = true;
                }
            }
            if nah {
                continue;
            }
            let p = v(i, j);
            for d in 0..2 {
                zeilen.push((vec![(2 * idx(i, j) + d, 1.0)], p[d], ALPHA));
            }
        }
    }
    // Es (Gl. 6–7): je Ecke die acht Dreiecke der vier angrenzenden Zellen.
    // Im regelmaessigen Vorverformungsnetz liegen die Koordinaten (u, v) fest;
    // berechnet werden sie trotzdem allgemein nach Gl. 6.
    for cj in 0..ny {
        for ci in 0..nx {
            let ws = auffaellig(ox + ci as f64 * zelle, oy + cj as f64 * zelle, zelle, zelle) + W_S_EPSILON;
            let ecken = [(ci, cj), (ci + 1, cj), (ci + 1, cj + 1), (ci, cj + 1)];
            for k in 0..4 {
                let v1 = ecken[k];
                let (na, nb, gegen) = (ecken[(k + 1) % 4], ecken[(k + 3) % 4], ecken[(k + 2) % 4]);
                for (v2, v3) in [(na, gegen), (gegen, nb)] {
                    let (p1, p2, p3) = (v(v1.0, v1.1), v(v2.0, v2.1), v(v3.0, v3.1));
                    let e = [p3[0] - p2[0], p3[1] - p2[1]];
                    let r = [e[1], -e[0]]; // R90 * e mit R90 = [0 1; −1 0]
                    let q = [p1[0] - p2[0], p1[1] - p2[1]];
                    let ee = e[0] * e[0] + e[1] * e[1];
                    let u = (q[0] * e[0] + q[1] * e[1]) / ee;
                    let w = (q[0] * r[0] + q[1] * r[1]) / ee;
                    // V1 − (V2 + u (V3 − V2) + w R90 (V3 − V2)) = 0, je Komponente.
                    let (i1, i2, i3) = (idx(v1.0, v1.1), idx(v2.0, v2.1), idx(v3.0, v3.1));
                    // x: V1x − V2x − u (V3x − V2x) − w (V3y − V2y)
                    zeilen.push((vec![(2 * i1, 1.0), (2 * i2, -1.0 + u), (2 * i3, -u), (2 * i2 + 1, w), (2 * i3 + 1, -w)], 0.0, BETA * ws));
                    // y: V1y − V2y − u (V3y − V2y) + w (V3x − V2x)
                    zeilen.push((vec![(2 * i1 + 1, 1.0), (2 * i2 + 1, -1.0 + u), (2 * i3 + 1, -u), (2 * i2, -w), (2 * i3, w)], 0.0, BETA * ws));
                }
            }
        }
    }
    // Normalgleichungen (duenn): A^T W A x = A^T W b, geloest per CG.
    let n = 2 * ecken_n;
    let mut diag = vec![0.0f64; n];
    let mut nb: Vec<std::collections::HashMap<usize, f64>> = vec![std::collections::HashMap::new(); n];
    let mut rechts = vec![0.0f64; n];
    for (k, b, w) in &zeilen {
        for &(a, ca) in k {
            rechts[a] += w * ca * b;
            for &(c, cc) in k {
                if a == c {
                    diag[a] += w * ca * cc;
                } else {
                    *nb[a].entry(c).or_insert(0.0) += w * ca * cc;
                }
            }
        }
    }
    let nb: Vec<Vec<(usize, f64)>> = nb.into_iter().map(|m| m.into_iter().collect()).collect();
    let mal = |x: &[f64], y: &mut [f64]| {
        for a in 0..n {
            let mut s = diag[a] * x[a];
            for &(c, v) in &nb[a] {
                s += v * x[c];
            }
            y[a] = s;
        }
    };
    // Start: Vorverformung.
    let mut x = vec![0.0f64; n];
    for j in 0..=ny {
        for i in 0..=nx {
            let p = v(i, j);
            x[2 * idx(i, j)] = p[0];
            x[2 * idx(i, j) + 1] = p[1];
        }
    }
    let mut ax = vec![0.0; n];
    mal(&x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|a| rechts[a] - ax[a]).collect();
    let pre: Vec<f64> = diag.iter().map(|d| if *d > 0.0 { 1.0 / d } else { 1.0 }).collect();
    let mut zv: Vec<f64> = r.iter().zip(&pre).map(|(a, b)| a * b).collect();
    let mut p = zv.clone();
    let mut rz: f64 = r.iter().zip(&zv).map(|(a, b)| a * b).sum();
    let norm0 = rechts.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    for _ in 0..20 * n {
        mal(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if pap <= 0.0 {
            break;
        }
        let a = rz / pap;
        for k in 0..n {
            x[k] += a * p[k];
            r[k] -= a * ap[k];
        }
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() < 1e-12 * norm0 {
            break;
        }
        for k in 0..n {
            zv[k] = r[k] * pre[k];
        }
        let rz_neu: f64 = r.iter().zip(&zv).map(|(a, b)| a * b).sum();
        let bb = rz_neu / rz;
        rz = rz_neu;
        for k in 0..n {
            p[k] = zv[k] + bb * p[k];
        }
    }
    Netz { x0: ox, y0: oy, zelle, nx, ny, ecken: (0..ecken_n).map(|k| [x[2 * k], x[2 * k + 1]]).collect() }
}

/// Parallaxe ausgleichen. `l`: volle Leinwand, `klein`: Leinwand der
/// Bewertung (Naht-Massstab), `mitte`: unverformtes Bild (Index in `bilder`),
/// `abstand`: Abstand jedes Bildes zur Mitte im Paargraphen,
/// `px_je_eingabepixel`: Leinwandpixel je Eingabepixel.
pub fn ausgleichen(l: &Leinwand, bilder: &[Bild], paare: &[Paar], abstand: &[usize], px_je_eingabepixel: f64, merkmal_massstab: f64) -> Ausgleich {
    let n = bilder.len();
    let mut netze: Vec<Option<Netz>> = vec![None; n];
    let schwelle = SCHWELLE_PX * px_je_eingabepixel;
    let pass = PASS_MERKMALSPIXEL / merkmal_massstab * px_je_eingabepixel;
    let mut vorher_alle = Vec::new();
    let mut nachher_alle = Vec::new();
    let mut behandelt = 0;
    // Paare mit messbarem Restversatz, bewegtes Bild = weiter von der Mitte.
    let mut auftraege: Vec<(usize, usize, Vec<Punkt>)> = Vec::new(); // (bewegt, bezug, punkte bewegt->bezug)
    for p in paare {
        let mut d: Vec<f64> = p.punkte.iter().map(|q| (q.a.0 - q.b.0).hypot(q.a.1 - q.b.1)).collect();
        let med = quantil(&mut d, 0.5);
        super::diagnose(&format!(
            "Parallaxe Paar {}-{}: {} Punkte, Restversatz Median {:.2}, 75 % {:.2}, 90 % {:.2} Leinwandpx",
            p.i, p.j, d.len(), med, quantil(&mut d, 0.75), quantil(&mut d, 0.9)
        ));
        if med <= schwelle {
            continue;
        }
        let (bewegt, bezug, pkt) = if (abstand[p.i], p.i) > (abstand[p.j], p.j) {
            (p.i, p.j, p.punkte.clone())
        } else {
            (p.j, p.i, p.punkte.iter().map(|q| Punkt { a: q.b, b: q.a }).collect())
        };
        auftraege.push((bewegt, bezug, pkt));
    }
    let mut reihenfolge: Vec<usize> = (0..n).collect();
    reihenfolge.sort_by_key(|&k| (abstand[k], k));
    for &m in &reihenfolge {
        let eigene: Vec<&(usize, usize, Vec<Punkt>)> = auftraege.iter().filter(|a| a.0 == m).collect();
        if eigene.is_empty() {
            continue;
        }
        let bm = &bilder[m];
        let (x0, x1, y0, y1) = bm.rechteck;
        let ecken = [(x0 as f64, y0 as f64), (x1 as f64, y0 as f64), (x0 as f64, y1 as f64), (x1 as f64, y1 as f64)];
        let diag = ((x1 - x0) as f64).hypot((y1 - y0) as f64);
        let zelle = ((x1 - x0) as f64).max((y1 - y0) as f64) / ZELLEN as f64;
        let mut ziele: Vec<(P2, P2)> = Vec::new();
        for (_, r, pkt) in eigene {
            let r = *r;
            // Ziele im endgueltigen Bezugsbild.
            let pkt: Vec<Punkt> = pkt.iter().map(|q| Punkt { a: q.a, b: endgueltig(&netze[r], q.b) }).collect();
            // Bewertungsleinwand ueber der Ueberlappung (Rechteckschnitt).
            let br = bilder[r].rechteck;
            let (ux0, ux1, uy0, uy1) = (x0.max(br.0), x1.min(br.1), y0.max(br.2), y1.min(br.3));
            if ux0 >= ux1 || uy0 >= uy1 {
                continue;
            }
            let flaeche = ((ux1 - ux0) * (uy1 - uy0)) as f64;
            let mb = (BEWERTUNG_PIXEL / flaeche).sqrt().min(1.0);
            let (bw, bh) = ((((ux1 - ux0) as f64) * mb).ceil() as usize, (((uy1 - uy0) as f64) * mb).ceil() as usize);
            let (ox, oy) = (ux0 as f64, uy0 as f64);
            let ohne = |x: f64, y: f64| (x, y);
            let (ga, oka, _) = grau_auf_leinwand(l, bm, mb, ox, oy, bw, bh, &ohne);
            let netz_r = netze[r].clone();
            let zur = move |x: f64, y: f64| match &netz_r {
                Some(nz) => nz.zurueck(x, y),
                None => (x, y),
            };
            let (gb, okb, _) = grau_auf_leinwand(l, &bilder[r], mb, ox, oy, bw, bh, &zur);
            // Kantenkarten, geglaettet mit der Passschwelle als σ (die Bewertung
            // soll Versatz bis zur Lockerheit der Homographie dulden).
            let sigma_e = (pass * mb).max(0.5);
            let ka = gauss_glatt(&canny(&ga, &oka, bw, bh), bw, bh, sigma_e);
            let kb = gauss_glatt(&canny(&gb, &okb, bw, bh), bw, bh, sigma_e);
            let bewertung = Bewertung { bw, bh, m: mb, ox, oy, kante_a: ka, ok_a: oka, kante_b: &kb, ok_b: &okb, sigma_g: zelle * mb };
            let gewaehlt = suchen(&pkt, &bewertung, &ecken, diag, pass, (m * 31 + r) as u64 + 5);
            super::diagnose(&format!("Parallaxe {m} -> {r}: {} von {} Merkmalen gewaehlt", gewaehlt.len(), pkt.len()));
            if gewaehlt.is_empty() {
                continue;
            }
            vorher_alle.extend(pkt.iter().map(|q| (q.a.0 - q.b.0).hypot(q.a.1 - q.b.1)));
            for k in gewaehlt {
                ziele.push((pkt[k].a, pkt[k].b));
            }
            behandelt += 1;
        }
        if ziele.is_empty() {
            continue;
        }
        // Auffaelligkeit (Liu 2009): L2-Norm der Farbvarianz der Zelle, auf
        // einer verkleinerten Kopie des Bildes auf der Leinwand (0..255).
        let ma = (BEWERTUNG_PIXEL / ((x1 - x0) * (y1 - y0)) as f64).sqrt().min(1.0) * 2.0;
        let ma = ma.min(1.0);
        let (aw, ah) = ((((x1 - x0) as f64) * ma).ceil() as usize, (((y1 - y0) as f64) * ma).ceil() as usize);
        let ohne = |x: f64, y: f64| (x, y);
        let (_, okf, farbe) = grau_auf_leinwand(l, bm, ma, x0 as f64, y0 as f64, aw, ah, &ohne);
        let auffaellig = |cx: f64, cy: f64, cw: f64, ch: f64| -> f64 {
            let (ax0, ay0) = (((cx - x0 as f64) * ma).floor().max(0.0) as usize, ((cy - y0 as f64) * ma).floor().max(0.0) as usize);
            let (ax1, ay1) = ((((cx + cw - x0 as f64) * ma).ceil() as usize).min(aw), (((cy + ch - y0 as f64) * ma).ceil() as usize).min(ah));
            let (mut s, mut q, mut z) = ([0.0f64; 3], [0.0f64; 3], 0.0);
            for y in ay0..ay1 {
                for x in ax0..ax1 {
                    let i = y * aw + x;
                    if !okf[i] {
                        continue;
                    }
                    for c in 0..3 {
                        s[c] += farbe[i][c] as f64;
                        q[c] += (farbe[i][c] as f64).powi(2);
                    }
                    z += 1.0;
                }
            }
            if z < 2.0 {
                return 0.0;
            }
            let var: Vec<f64> = (0..3).map(|c| q[c] / z - (s[c] / z).powi(2)).collect();
            (var[0] * var[0] + var[1] * var[1] + var[2] * var[2]).sqrt()
        };
        let netz = netz_loesen(bm.rechteck, &ziele, &auffaellig);
        for (a, b) in &ziele {
            let q = netz.vor(a.0, a.1);
            nachher_alle.push((q.0 - b.0).hypot(q.1 - b.1));
        }
        super::diagnose(&format!("Parallaxe Bild {m}: Netz {}x{}, groesste Verschiebung {:.1} px, Neigung {:.2} Grad", netz.nx, netz.ny, netz.groesste(), netz.groesste_neigung_grad()));
        netze[m] = Some(netz);
    }
    let vorher = (quantil(&mut vorher_alle.clone(), 0.5), quantil(&mut vorher_alle, 0.9));
    let nachher = (quantil(&mut nachher_alle.clone(), 0.5), quantil(&mut nachher_alle, 0.9));
    Ausgleich { netze, vorher, nachher, paare: behandelt }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn umkehrung_bilinear() {
        let (a, b, c, d) = ([0.0, 0.0], [10.0, 1.0], [-1.0, 9.0], [11.0, 12.0]);
        for &(u, v) in &[(0.2, 0.3), (0.9, 0.1), (0.5, 0.5), (0.0, 1.0)] {
            let p = [
                (1.0 - u) * (1.0 - v) * a[0] + u * (1.0 - v) * b[0] + (1.0 - u) * v * c[0] + u * v * d[0],
                (1.0 - u) * (1.0 - v) * a[1] + u * (1.0 - v) * b[1] + (1.0 - u) * v * c[1] + u * v * d[1],
            ];
            let (uu, vv) = umgekehrt_bilinear(a, b, c, d, p);
            assert!((uu - u).abs() < 1e-9 && (vv - v).abs() < 1e-9, "{u} {v} -> {uu} {vv}");
        }
    }

    /// Ohne Merkmale bleibt das Netz die Vorverformung; mit einem Merkmal wird
    /// es dorthin gezogen, Vor- und Rueckabbildung sind zueinander invers.
    #[test]
    fn netz_folgt_merkmalen_und_ist_umkehrbar() {
        let flach = |_: f64, _: f64, _: f64, _: f64| 0.0;
        let n = netz_loesen((0, 640, 0, 480), &[], &flach);
        assert!(n.groesste() < 1e-6, "{}", n.groesste());
        let ziele: Vec<(P2, P2)> = (0..5).map(|k| ((300.0 + 5.0 * k as f64, 240.0), (304.0 + 5.0 * k as f64, 242.0))).collect();
        let n = netz_loesen((0, 640, 0, 480), &ziele, &flach);
        for (a, b) in &ziele {
            let q = n.vor(a.0, a.1);
            assert!((q.0 - b.0).hypot(q.1 - b.1) < 0.5, "{q:?} {b:?}");
        }
        for &(x, y) in &[(100.0, 100.0), (310.0, 245.0), (600.0, 400.0)] {
            let q = n.vor(x, y);
            let r = n.zurueck(q.0, q.1);
            assert!((r.0 - x).hypot(r.1 - y) < 1e-6, "{x} {y} -> {q:?} -> {r:?}");
        }
        // Fernab der Merkmale (Eg): praktisch unverformt.
        let q = n.vor(20.0, 20.0);
        assert!((q.0 - 20.0).hypot(q.1 - 20.0) < 0.5, "{q:?}");
    }
}
