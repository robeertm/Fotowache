//! Merkmale finden und zuordnen — auf einer verkleinerten Graustufenkopie.
//!
//! ## Warum so
//! - **Harris-Ecken** statt Kantenpunkten: eine Ecke ist in beiden Richtungen
//!   festgelegt, ein Kantenpunkt nur quer zur Kante. Fuer eine Homographie
//!   braucht man Punkte, die sich nicht entlang einer Linie verschieben lassen.
//! - **Aufrechte Binaerdeskriptoren** (256 Helligkeitsvergleiche in einem Feld um
//!   den Punkt), auf mehreren Massstaeben: Vergleiche sind
//!   unempfindlich gegen Belichtungsunterschiede zwischen den Aufnahmen, und der
//!   Abstand ist ein Bitzaehlen — schnell genug, um bei 30 Bildern ALLE Paare
//!   roh gegeneinander zu pruefen. Das ist der Grund, warum die Reihenfolge der
//!   Bilder egal ist.
//! - **Wahrnehmungsnahe Helligkeit** (Gamma 1/2.2) statt linearer Werte: in
//!   linearen Daten liegt fast alles in den untersten Prozenten, Schattenzeichnung
//!   haette keine messbaren Ecken.
//! - **Gleichmaessig verteilt** (Raster): sonst saemmeln sich alle Punkte im
//!   kontrastreichsten Bilddrittel, und die Ueberlappung zum Nachbarn bleibt leer.

use super::mathe::Zufall;

/// Graustufenbild, zeilenweise.
pub struct Grau {
    pub w: usize,
    pub h: usize,
    pub d: Vec<f32>,
}

impl Grau {
    #[inline]
    pub fn px(&self, x: usize, y: usize) -> f32 {
        self.d[y * self.w + x]
    }
}

/// Ein Merkmalspunkt in Pixelkoordinaten (Pixelmitte bei +0.5).
#[derive(Clone, Copy, Debug)]
pub struct Punkt {
    pub x: f32,
    pub y: f32,
}

pub struct Merkmale {
    pub punkte: Vec<Punkt>,
    pub desk: Vec<[u64; 4]>,
}

/// Flaechentreues Verkleinern eines verschraenkten Puffers mit `k` Kanaelen.
/// Ausgabepixel i deckt exakt [i/s, (i+1)/s) der Eingabe ab (Randpixel anteilig) —
/// dadurch gilt x_klein = s * x_gross ohne halbe-Pixel-Versatz, und die feine
/// Ausrichtung auf voller Aufloesung passt nahtlos an die grobe an.
pub fn verkleinern(d: &[f32], w: usize, h: usize, k: usize, s: f64) -> (Vec<f32>, usize, usize) {
    let ow = ((w as f64 * s).floor() as usize).max(1);
    let oh = ((h as f64 * s).floor() as usize).max(1);
    let gewichte = |n_aus: usize, n_ein: usize| -> Vec<(usize, Vec<f32>)> {
        (0..n_aus)
            .map(|i| {
                let a = i as f64 / s;
                let b = ((i + 1) as f64 / s).min(n_ein as f64);
                let start = a.floor() as usize;
                let ende = (b.ceil() as usize).min(n_ein);
                let mut g = Vec::with_capacity(ende - start);
                for j in start..ende {
                    let lo = (j as f64).max(a);
                    let hi = ((j + 1) as f64).min(b);
                    g.push(((hi - lo).max(0.0) * s) as f32);
                }
                (start, g)
            })
            .collect()
    };
    let gx = gewichte(ow, w);
    let gy = gewichte(oh, h);
    use rayon::prelude::*;
    // Erst waagrecht (Zeile fuer Zeile), dann senkrecht — getrennt ist das ein
    // Bruchteil der Rechenarbeit eines 2D-Kastens.
    let mut zwischen = vec![0.0f32; ow * h * k];
    zwischen.par_chunks_mut(ow * k).enumerate().for_each(|(y, zeile)| {
        let ein = &d[y * w * k..(y + 1) * w * k];
        for (x, (start, g)) in gx.iter().enumerate() {
            for c in 0..k {
                let mut s = 0.0f32;
                for (o, gw) in g.iter().enumerate() {
                    s += ein[(start + o) * k + c] * gw;
                }
                zeile[x * k + c] = s;
            }
        }
    });
    let mut aus = vec![0.0f32; ow * oh * k];
    aus.par_chunks_mut(ow * k).enumerate().for_each(|(y, zeile)| {
        let (start, g) = &gy[y];
        for (o, gw) in g.iter().enumerate() {
            let ein = &zwischen[(start + o) * ow * k..(start + o + 1) * ow * k];
            for (a, e) in zeile.iter_mut().zip(ein) {
                *a += e * gw;
            }
        }
    });
    (aus, ow, oh)
}

/// Wahrnehmungsnahe Helligkeit aus linearem RGB.
#[inline]
pub fn helligkeit(r: f32, g: f32, b: f32) -> f32 {
    (0.3 * r + 0.55 * g + 0.15 * b).max(0.0).powf(1.0 / 2.2)
}

pub fn grau_aus_rgb(rgb: &[f32], w: usize, h: usize) -> Grau {
    let d = rgb.chunks_exact(3).map(|p| helligkeit(p[0], p[1], p[2])).collect::<Vec<_>>();
    debug_assert_eq!(d.len(), w * h);
    Grau { w, h, d }
}

/// Binomialfilter [1 4 6 4 1]/16 (Sigma ~1), Rand geklemmt.
pub fn weichzeichnen(g: &Grau) -> Grau {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let (w, h) = (g.w, g.h);
    let mut t = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (o, kv) in K.iter().enumerate() {
                let xx = (x as isize + o as isize - 2).clamp(0, w as isize - 1) as usize;
                s += g.d[y * w + xx] * kv;
            }
            t[y * w + x] = s;
        }
    }
    let mut d = vec![0.0f32; w * h];
    for y in 0..h {
        for (o, kv) in K.iter().enumerate() {
            let yy = (y as isize + o as isize - 2).clamp(0, h as isize - 1) as usize;
            let z = &t[yy * w..(yy + 1) * w];
            for x in 0..w {
                d[y * w + x] += z[x] * kv;
            }
        }
    }
    Grau { w, h, d }
}

/// Feldradius des Deskriptors und Abstand vom Bildrand.
const FELD: f32 = 13.0;
const RAND: usize = 16;

/// Vergleichsmuster: 256 Punktpaare, normalverteilt im Feld (fest gesaet,
/// damit alle Bilder dasselbe Muster benutzen).
fn muster() -> &'static [[f32; 4]; 256] {
    use std::sync::OnceLock;
    static M: OnceLock<[[f32; 4]; 256]> = OnceLock::new();
    M.get_or_init(|| {
        let mut z = Zufall::neu(0x5041_4e4f);
        let mut m = [[0.0f32; 4]; 256];
        for p in m.iter_mut() {
            for v in p.iter_mut() {
                *v = ((z.normal() * FELD as f64 / 2.2) as f32).clamp(-FELD, FELD);
            }
        }
        m
    })
}

/// Findet bis zu `anzahl` Merkmale und beschreibt sie.
pub fn finden(g: &Grau, anzahl: usize) -> Merkmale {
    let (w, h) = (g.w, g.h);
    if w < 2 * RAND + 8 || h < 2 * RAND + 8 {
        return Merkmale { punkte: vec![], desk: vec![] };
    }
    let glatt = weichzeichnen(g);
    // Harris-Antwort aus den Gradienten; das Fenster ist ein zweiter
    // Binomialfilter auf den Gradientenprodukten.
    let mut ixx = Grau { w, h, d: vec![0.0; w * h] };
    let mut iyy = Grau { w, h, d: vec![0.0; w * h] };
    let mut ixy = Grau { w, h, d: vec![0.0; w * h] };
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = 0.5 * (glatt.px(x + 1, y) - glatt.px(x - 1, y));
            let gy = 0.5 * (glatt.px(x, y + 1) - glatt.px(x, y - 1));
            let i = y * w + x;
            ixx.d[i] = gx * gx;
            iyy.d[i] = gy * gy;
            ixy.d[i] = gx * gy;
        }
    }
    let (ixx, iyy, ixy) = (weichzeichnen(&weichzeichnen(&ixx)), weichzeichnen(&weichzeichnen(&iyy)), weichzeichnen(&weichzeichnen(&ixy)));
    let mut r = vec![0.0f32; w * h];
    for (((v, a), b), c) in r.iter_mut().zip(&ixx.d).zip(&iyy.d).zip(&ixy.d) {
        *v = a * b - c * c - 0.04 * (a + b) * (a + b);
    }
    let rmax = r.iter().cloned().fold(0.0f32, f32::max);
    if rmax <= 0.0 {
        return Merkmale { punkte: vec![], desk: vec![] };
    }
    // Schwelle relativ zum staerksten Punkt: Rauschen im Himmel liefert winzige,
    // zufaellige Ecken, die nirgends wiedergefunden werden.
    let schwelle = rmax * 2e-5;
    // Raster fuer gleichmaessige Verteilung.
    let zellen_x = 12usize;
    let zellen_y = ((zellen_x as f32 * h as f32 / w as f32).round() as usize).clamp(4, 24);
    let je_zelle = (anzahl as f32 / (zellen_x * zellen_y) as f32 * 1.6).ceil() as usize;
    let mut zellen: Vec<Vec<(f32, usize, usize)>> = vec![Vec::new(); zellen_x * zellen_y];
    for y in RAND..h - RAND {
        for x in RAND..w - RAND {
            let v = r[y * w + x];
            if v < schwelle {
                continue;
            }
            // Lokales Maximum im 5x5-Fenster.
            let mut max = true;
            'aussen: for dy in -2isize..=2 {
                for dx in -2isize..=2 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let n = r[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
                    if n > v || (n == v && (dy < 0 || (dy == 0 && dx < 0))) {
                        max = false;
                        break 'aussen;
                    }
                }
            }
            if max {
                let zx = (x * zellen_x / w).min(zellen_x - 1);
                let zy = (y * zellen_y / h).min(zellen_y - 1);
                zellen[zy * zellen_x + zx].push((v, x, y));
            }
        }
    }
    let mut kandidaten = Vec::new();
    for z in zellen.iter_mut() {
        z.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        kandidaten.extend(z.iter().take(je_zelle).copied());
    }
    kandidaten.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    kandidaten.truncate(anzahl);

    // Deskriptoren auf einer staerker geglaetteten Kopie: Bitvergleiche einzelner
    // Pixel waeren sonst Rauschvergleiche.
    let feld = weichzeichnen(&glatt);
    let m = muster();
    let mut punkte = Vec::with_capacity(kandidaten.len());
    let mut desk = Vec::with_capacity(kandidaten.len());
    for &(_, x, y) in &kandidaten {
        // Unterpixelgenau per Parabel durch die Antwort.
        let sub = |a: f32, b: f32, c: f32| {
            let n = a - 2.0 * b + c;
            if n.abs() < 1e-20 {
                0.0
            } else {
                (0.5 * (a - c) / n).clamp(-0.5, 0.5)
            }
        };
        let dx = sub(r[y * w + x - 1], r[y * w + x], r[y * w + x + 1]);
        let dy = sub(r[(y - 1) * w + x], r[y * w + x], r[(y + 1) * w + x]);
        // Aufrecht, nicht nach einer geschaetzten Richtung gedreht:
        // Panoramabilder sind (aufgerichtet) kaum gegeneinander verdreht, die
        // Richtungsschaetzung ist auf Schicht- und Rastermustern dagegen
        // verrauscht. Gemessen an echten Reihen: aufrecht 10-30 % mehr stimmige
        // Zuordnungen. Die Vergleiche vertragen ein paar Grad Rollen.
        let mut d = [0u64; 4];
        for (k, p) in m.iter().enumerate() {
            let probe = |px: f32, py: f32| feld.px((px + x as f32).round() as usize, (py + y as f32).round() as usize);
            if probe(p[0], p[1]) < probe(p[2], p[3]) {
                d[k >> 6] |= 1u64 << (k & 63);
            }
        }
        punkte.push(Punkt { x: x as f32 + dx + 0.5, y: y as f32 + dy + 0.5 });
        desk.push(d);
    }
    Merkmale { punkte, desk }
}

#[inline]
pub fn abstand(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (a[0] ^ b[0]).count_ones() + (a[1] ^ b[1]).count_ones() + (a[2] ^ b[2]).count_ones() + (a[3] ^ b[3]).count_ones()
}

/// Rohe Zuordnung zweier Merkmalsmengen: beidseitig bester Partner,
/// Verhaeltnistest gegen den Zweitbesten (verwirft Mehrdeutiges wie
/// Fensterreihen), absolute Obergrenze gegen reine Zufallstreffer.
pub fn zuordnen(a: &Merkmale, b: &Merkmale) -> Vec<(usize, usize)> {
    if a.desk.is_empty() || b.desk.is_empty() {
        return vec![];
    }
    let mut best_a = vec![(u32::MAX, u32::MAX, 0usize); a.desk.len()];
    let mut best_b = vec![(u32::MAX, 0usize); b.desk.len()];
    for (i, da) in a.desk.iter().enumerate() {
        let (mut d1, mut d2, mut j1) = (u32::MAX, u32::MAX, 0usize);
        for (j, db) in b.desk.iter().enumerate() {
            let d = abstand(da, db);
            if d < d1 {
                d2 = d1;
                d1 = d;
                j1 = j;
            } else if d < d2 {
                d2 = d;
            }
            if d < best_b[j].0 {
                best_b[j] = (d, i);
            }
        }
        best_a[i] = (d1, d2, j1);
    }
    let mut paare = Vec::new();
    for (i, &(d1, d2, j)) in best_a.iter().enumerate() {
        if d1 <= 70 && (d1 as f32) < 0.8 * d2 as f32 && best_b[j].1 == i {
            paare.push((i, j));
        }
    }
    paare
}

/// Merkmale auf mehreren Massstaeben (Stufen je Faktor 0,75), Koordinaten in
/// der Ausgangskopie. Gemessen an einer echten Reihe: wer zwischen zwei
/// Aufnahmen ein paar Schritte naeher geht, sieht dieselbe Felswand 1,3- bis
/// 1,5-fach groesser — die Vergleichsfelder eines einzigen Massstabs finden dann
/// nichts wieder. Ueber die Stufen hinweg trifft ein Feld im einen Bild sein
/// Gegenstueck im anderen bis auf ~15 % Groessenunterschied, und das vertragen
/// die Vergleiche.
pub fn finden_mehrstufig(g: &Grau, anzahl: usize) -> Merkmale {
    const STUFEN: [f64; 4] = [1.0, 0.75, 0.5625, 0.421875];
    let flaeche: f64 = STUFEN.iter().map(|s| s * s).sum();
    let mut punkte = Vec::new();
    let mut desk = Vec::new();
    for &st in &STUFEN {
        let n = ((anzahl as f64) * st * st / flaeche).round() as usize;
        let m = if st == 1.0 {
            finden(g, n)
        } else {
            let (d, w, h) = verkleinern(&g.d, g.w, g.h, 1, st);
            finden(&Grau { w, h, d }, n)
        };
        for (p, d) in m.punkte.into_iter().zip(m.desk) {
            punkte.push(Punkt { x: (p.x as f64 / st) as f32, y: (p.y as f64 / st) as f32 });
            desk.push(d);
        }
    }
    Merkmale { punkte, desk }
}

/// Mehrfachkandidaten fuer den Rueckfallweg: je Punkt aus `a` die `k` naechsten
/// aus `b` (bis `max_abstand`), OHNE Eindeutigkeitspruefung — bei Fensterrastern
/// ist der richtige Partner oft nur der zweitbeste. Die Geometrie entscheidet.
/// Rueckgabe (ia, ib, Guete); kleinere Guete = verlaesslicher (d1/d2 fuer den
/// besten, 1 fuer weitere).
pub fn kandidaten(a: &Merkmale, b: &Merkmale, k: usize, max_abstand: u32) -> Vec<(usize, usize, f32)> {
    let mut aus = Vec::new();
    if b.desk.is_empty() {
        return aus;
    }
    let mut beste: Vec<(u32, usize)> = Vec::with_capacity(k + 1);
    for (ia, da) in a.desk.iter().enumerate() {
        beste.clear();
        for (ib, db) in b.desk.iter().enumerate() {
            let d = abstand(da, db);
            if d > max_abstand {
                continue;
            }
            if beste.len() < k + 1 || d < beste[beste.len() - 1].0 {
                let pos = beste.partition_point(|e| e.0 <= d);
                beste.insert(pos, (d, ib));
                beste.truncate(k + 1);
            }
        }
        for (r, &(d, ib)) in beste.iter().take(k).enumerate() {
            let guete = if r == 0 {
                beste.get(1).map_or(0.5, |n| d as f32 / n.0.max(1) as f32)
            } else {
                1.0
            };
            aus.push((ia, ib, guete));
        }
    }
    aus
}
