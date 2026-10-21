//! Belichtungsausgleich und Nahtsuche auf einer kleinen Leinwand.
//!
//! Beides braucht nur grobe Bilder: Helligkeitsverhaeltnisse aendern sich mit der
//! Aufloesung nicht, und eine Naht muss nur grob an bewegten Dingen vorbei laufen —
//! die Mehrband-Mischung glaettet danach den Uebergang auf voller Aufloesung.
//! Die Groesse der kleinen Leinwand (`NAHT_PIXEL` in mod.rs) ist bewusst groesser
//! als die 0,1 MP der Referenzumsetzung von Brown & Lowe (seam_megapix).
//!
//! ## Belichtung (Brown & Lowe, IJCV 2007, Abschnitt 6, Gl. 27–29)
//! e = ½ Σ_i Σ_j N_ij ((g_i Ī_ij − g_j Ī_ji)² / σN² + (1 − g_i)² / σg²), geschlossen
//! geloest (Ableitung null; die Summe laeuft wie in Gl. 29 auch ueber j = i,
//! wie in der Referenzumsetzung). σN = 10 bezieht sich auf Bildwerte 0..255 —
//! gammakodierte 8-bit-Werte, wie sie die Arbeit verarbeitet. Unsere Daten sind
//! linear; deshalb werden die Mittel Ī_ij auf derselben Skala gebildet
//! (I = 255 · L^(1/2,2), L = lineare Helligkeit) und die Gleichung dort
//! geloest. Eine Verstaerkung g auf diesen Werten ist exakt die lineare
//! Verstaerkung g^2,2 (reine Potenzfunktion: g · L^(1/γ) = (g^γ L)^(1/γ)),
//! damit wird umgerechnet — das Modell bleibt, die Verstaerkung wirkt auf die
//! linearen Daten.
//! **Abweichung σg:** B&L nehmen σg = 0,1. Der Zug zu 1 laesst dann einen Teil
//! des Belichtungsunterschieds stehen (gemessen an Pruefszenen 20–26 %); am
//! Pruefstand war der Farbunterschied entlang der Naht damit in 11 von 14
//! Reihen groesser als mit σg = 1,0 (z. B. 1669–1672 8,9 gegen 6,3; 2191–2194
//! 5,8 gegen 2,7). Mit σg = 1,0 erfuellt der Prior weiter den Zweck,
//! den die Arbeit nennt (die triviale Loesung g = 0 ausschliessen), gleicht
//! aber fast vollstaendig an.
//!
//! ## Startaufteilung (Gl. 31) und Naht (Graphenschnitt)
//! Jedes Pixel gehoert zuerst dem Bild mit dem groessten Gewicht
//! W(x, y) = w(x) w(y), w linear von 1 in der Bildmitte auf 0 am Rand (Gl. 30/31).
//! Danach wird jede Naht per Graphenschnitt nach Kwatra u. a. 2003 ("Graphcut
//! Textures") neu gelegt, wie die Nahtsuche der Referenzumsetzung von
//! Brown & Lowe: je Bildpaar (in Indexreihenfolge) ueber die GANZE gemeinsame
//! Flaeche, die einem der beiden gehoert; angrenzende Pixel, die nur zu einem
//! der beiden gehoeren, sind fest an Quelle bzw. Senke; Kantenkosten zwischen
//! Nachbarn s, t: |I1(s) − I2(s)|² + |I1(t) − I2(t)|² + 1 (reine Farbkosten,
//! die Voreinstellung des Beispielablaufs der Referenzumsetzung), Bildwerte 0..255
//! (gammakodiert wie oben, nach Belichtungsausgleich). Gemessen gegen
//! Farb-Gradienten-Kosten (Teilung durch die Sobel-Gradienten): der
//! Farbunterschied entlang der Naht ist mit reinen Farbkosten in 12 von 14
//! Reihen kleiner, oft um die
//! Haelfte (2094–2098: 7,3 gegen 16,8; 2565–2569: 4,6 gegen 10,9) — der
//! Gradiententeiler macht Schnitte quer durch kontrastreiche Kanten billig,
//! gerade dort fallen aber Doppelkonturen auf.

use super::projektion::{Kamera, Leinwand};
use rayon::prelude::*;

/// Ein Bild auf der kleinen Leinwand.
pub struct KleinBild {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    /// Linear RGB; auch ausserhalb des Bildes randverlaengert belegt (damit die
    /// Ableitungen am Bildrand keine kuenstliche Kante sehen).
    pub rgb: Vec<f32>,
    pub gueltig: Vec<bool>,
    /// Gewicht W = w(x) w(y) (B&L Gl. 30), 0 ausserhalb.
    pub gewicht: Vec<f32>,
}

impl KleinBild {
    #[inline]
    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return None;
        }
        let i = (y - self.y0) * self.w + (x - self.x0);
        self.gueltig[i].then_some(i)
    }
    #[inline]
    fn index_roh(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return None;
        }
        Some((y - self.y0) * self.w + (x - self.x0))
    }
}

/// Bilineares Abtasten eines verschraenkten RGB-Puffers mit Randklemmung.
#[inline]
pub fn abtasten(d: &[f32], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    // Pixelmitte bei +0.5
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f32);
    let x0 = fx.floor() as usize;
    let y0 = fy.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let ax = fx - x0 as f32;
    let ay = fy - y0 as f32;
    let mut o = [0.0f32; 3];
    let (i00, i10, i01, i11) = ((y0 * w + x0) * 3, (y0 * w + x1) * 3, (y1 * w + x0) * 3, (y1 * w + x1) * 3);
    for c in 0..3 {
        let a = d[i00 + c] + (d[i10 + c] - d[i00 + c]) * ax;
        let b = d[i01 + c] + (d[i11 + c] - d[i01 + c]) * ax;
        o[c] = a + (b - a) * ay;
    }
    o
}

/// Quellkoordinate (Pixel, Mitte bei +0.5) eines Leinwandpixels in einem Bild.
#[cfg(test)]
#[inline]
pub fn quelle(l: &Leinwand, k: &Kamera, x: usize, y: usize) -> Option<(f64, f64)> {
    quelle_f(l, k, x as f64 + 0.5, y as f64 + 0.5)
}

/// Wie `quelle`, fuer eine stetige Leinwandstelle (Pixelmitte bei +0.5).
#[inline]
pub fn quelle_f(l: &Leinwand, k: &Kamera, fx: f64, fy: f64) -> Option<(f64, f64)> {
    k.bildpunkt(&l.a.zurueck(l.u0 + fx, l.v0 + fy))
}

/// Gewicht w(x) w(y) nach B&L Gl. 30: linear von 1 in der Mitte auf 0 am Rand.
#[inline]
pub fn gewicht(k: &Kamera, sx: f64, sy: f64) -> f32 {
    let wx = 1.0 - (2.0 * sx / k.w - 1.0).abs();
    let wy = 1.0 - (2.0 * sy / k.h - 1.0).abs();
    (wx.max(0.0) * wy.max(0.0)) as f32
}

/// Bild auf die kleine Leinwand legen. `quelle` bildet eine Leinwandstelle
/// (volle Leinwand, stetig) auf die Quellkoordinate im Bild ab (Kamera, ggf.
/// mit oertlicher Verformung); `qs`: Massstab der Bilddaten `rgb` zur vollen
/// Aufloesung.
#[allow(clippy::too_many_arguments)]
pub fn verzerren_klein(
    l: &Leinwand,
    k: &Kamera,
    rgb: &[f32],
    qw: usize,
    qh: usize,
    qs: f64,
    sigma: f64,
    rechteck: (usize, usize, usize, usize),
    quelle: &(dyn Fn(f64, f64) -> Option<(f64, f64)> + Sync),
) -> KleinBild {
    let _ = l;
    let (x0, x1, y0, y1) = rechteck;
    let (w, h) = (x1 - x0, y1 - y0);
    let mut aus = vec![0.0f32; w * h * 3];
    let mut gueltig = vec![false; w * h];
    let mut gew = vec![0.0f32; w * h];
    aus.par_chunks_mut(w * 3).zip(gueltig.par_chunks_mut(w)).zip(gew.par_chunks_mut(w)).enumerate().for_each(|(yy, ((za, zg), zw))| {
        for xx in 0..w {
            let (fx, fy) = (((x0 + xx) as f64 + 0.5) / sigma, ((y0 + yy) as f64 + 0.5) / sigma);
            let Some((sx, sy)) = quelle(fx, fy) else { continue };
            let p = abtasten(rgb, qw, qh, (sx * qs) as f32, (sy * qs) as f32);
            za[xx * 3..xx * 3 + 3].copy_from_slice(&p);
            if sx < 0.5 || sy < 0.5 || sx > k.w - 0.5 || sy > k.h - 0.5 {
                continue;
            }
            zg[xx] = true;
            zw[xx] = gewicht(k, sx, sy);
        }
    });
    KleinBild { x0, y0, w, h, rgb: aus, gueltig, gewicht: gew }
}

/// Gammakodierter Bildwert 0..255 (wie die 8-bit-Daten der Arbeit).
#[inline]
fn kodiert(v: f32) -> f32 {
    255.0 * v.max(0.0).powf(1.0 / 2.2)
}

#[inline]
fn helligkeit(p: &[f32]) -> f32 {
    0.3 * p[0] + 0.55 * p[1] + 0.15 * p[2]
}

/// Verstaerkung je Bild nach B&L Gl. 29 (siehe Modulkopf), als LINEARE
/// Verstaerkung.
pub fn verstaerkungen(bilder: &[KleinBild]) -> Vec<f64> {
    const SIGMA_N: f64 = 10.0;
    // σg: B&L 0,1; hier 1,0 — siehe Modulkopf (gemessen).
    const SIGMA_G: f64 = 1.0;
    const GAMMA: f64 = 2.2;
    let n = bilder.len();
    let mut anz = vec![0.0f64; n * n];
    let mut mittel = vec![0.0f64; n * n]; // mittel[i*n+j] = Ī_ij
    let paare: Vec<(usize, usize)> = (0..n).flat_map(|i| (i..n).map(move |j| (i, j))).collect();
    let werte: Vec<(usize, usize, f64, f64, f64)> = paare
        .par_iter()
        .map(|&(i, j)| {
            let (a, b) = (&bilder[i], &bilder[j]);
            let xs = a.x0.max(b.x0);
            let xe = (a.x0 + a.w).min(b.x0 + b.w);
            let ys = a.y0.max(b.y0);
            let ye = (a.y0 + a.h).min(b.y0 + b.h);
            let (mut c, mut si, mut sj) = (0.0, 0.0, 0.0);
            for y in ys..ye {
                for x in xs..xe {
                    let (Some(ia), Some(ib)) = (a.index(x, y), b.index(x, y)) else { continue };
                    c += 1.0;
                    si += kodiert(helligkeit(&a.rgb[ia * 3..ia * 3 + 3])) as f64;
                    sj += kodiert(helligkeit(&b.rgb[ib * 3..ib * 3 + 3])) as f64;
                }
            }
            (i, j, c, si, sj)
        })
        .collect();
    for (i, j, c, si, sj) in werte {
        if c > 0.0 {
            anz[i * n + j] = c;
            anz[j * n + i] = c;
            mittel[i * n + j] = si / c;
            mittel[j * n + i] = sj / c;
        }
    }
    // Ableitung von Gl. 29 nach g_i gleich null:
    // Σ_j N_ij [ (g_i Ī_ij − g_j Ī_ji) Ī_ij · 2/σN² + (g_i − 1)/σg² ] = 0
    // (Faktor 2: das Paar (i, j) steht in der Doppelsumme zweimal).
    let mut a = vec![0.0f64; n * n];
    let mut b = vec![0.0f64; n];
    for i in 0..n {
        for j in 0..n {
            let nij = anz[i * n + j];
            if nij == 0.0 {
                continue;
            }
            a[i * n + i] += nij / (SIGMA_G * SIGMA_G);
            b[i] += nij / (SIGMA_G * SIGMA_G);
            if i != j {
                let (iij, iji) = (mittel[i * n + j], mittel[j * n + i]);
                a[i * n + i] += 2.0 * nij * iij * iij / (SIGMA_N * SIGMA_N);
                a[i * n + j] -= 2.0 * nij * iij * iji / (SIGMA_N * SIGMA_N);
            }
        }
    }
    for i in 0..n {
        if a[i * n + i] == 0.0 {
            a[i * n + i] = 1.0;
            b[i] = 1.0;
        }
    }
    let g = super::mathe::loesen(&mut a, &mut b, n).unwrap_or_else(|| vec![1.0; n]);
    g.iter().map(|v| if v.is_finite() && *v > 0.0 { v.powf(GAMMA) } else { 1.0 }).collect()
}

pub const LEER: u16 = u16::MAX;

/// Aufteilung der Leinwand auf die Bilder (siehe Modulkopf).
pub fn aufteilen(lw: usize, lh: usize, bilder: &[KleinBild], gain: &[f64]) -> Vec<u16> {
    let mut label = vec![LEER; lw * lh];
    let mut best = vec![-1.0f32; lw * lh];
    for (k, b) in bilder.iter().enumerate() {
        for y in 0..b.h {
            for x in 0..b.w {
                let i = y * b.w + x;
                if !b.gueltig[i] {
                    continue;
                }
                let g = (b.y0 + y) * lw + b.x0 + x;
                if b.gewicht[i] > best[g] {
                    best[g] = b.gewicht[i];
                    label[g] = k as u16;
                }
            }
        }
    }
    // Gammakodierte Werte (0..255) nach Belichtungsausgleich, je Bild auf
    // seinem Rechteck.
    let vorbereitet: Vec<Vec<f32>> = bilder.par_iter().zip(gain).map(|(b, g)| b.rgb.iter().map(|v| kodiert(v * *g as f32)).collect()).collect();
    let n = bilder.len();
    for i in 0..n {
        for j in i + 1..n {
            schnitt(lw, lh, bilder, &vorbereitet, &mut label, i, j);
        }
    }
    label
}

/// Graphenschnitt fuer ein Paar (siehe Modulkopf).
fn schnitt(lw: usize, lh: usize, bilder: &[KleinBild], v: &[Vec<f32>], label: &mut [u16], i: usize, j: usize) {
    let (a, b) = (&bilder[i], &bilder[j]);
    let xs = a.x0.max(b.x0);
    let xe = (a.x0 + a.w).min(b.x0 + b.w);
    let ys = a.y0.max(b.y0);
    let ye = (a.y0 + a.h).min(b.y0 + b.h);
    if xs >= xe || ys >= ye {
        return;
    }
    let (bw, bh) = (xe - xs, ye - ys);
    let (li, lj) = (i as u16, j as u16);
    let mut knoten = vec![u32::MAX; bw * bh];
    let mut lage: Vec<(usize, usize)> = Vec::new();
    for y in ys..ye {
        for x in xs..xe {
            let l = label[y * lw + x];
            if (l == li || l == lj) && a.index(x, y).is_some() && b.index(x, y).is_some() {
                knoten[(y - ys) * bw + (x - xs)] = lage.len() as u32;
                lage.push((x, y));
            }
        }
    }
    let nk = lage.len();
    if nk < 4 {
        return;
    }
    // Kantenkosten wie die reinen Farbkosten der Referenzumsetzung.
    let kosten = |s: (usize, usize), t: (usize, usize)| -> f32 {
        let (Some(as_), Some(at)) = (a.index_roh(s.0, s.1), a.index_roh(t.0, t.1)) else { return 1.0 };
        let (Some(bs), Some(bt)) = (b.index_roh(s.0, s.1), b.index_roh(t.0, t.1)) else { return 1.0 };
        let d2 = |ia: usize, ib: usize| {
            let (p, q) = (&v[i][ia * 3..ia * 3 + 3], &v[j][ib * 3..ib * 3 + 3]);
            (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)
        };
        d2(as_, bs) + d2(at, bt) + 1.0
    };
    const GROSS: i64 = 1 << 40;
    const SKALA: f32 = 1000.0;
    let mut g = Graph::neu(nk);
    for (k, &(x, y)) in lage.iter().enumerate() {
        let nachbarn = [(x as isize - 1, y as isize), (x as isize + 1, y as isize), (x as isize, y as isize - 1), (x as isize, y as isize + 1)];
        for (nx, ny) in nachbarn {
            if nx < 0 || ny < 0 || nx >= lw as isize || ny >= lh as isize {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let innen = nx >= xs && nx < xe && ny >= ys && ny < ye;
            let nkn = if innen { knoten[(ny - ys) * bw + (nx - xs)] } else { u32::MAX };
            if nkn != u32::MAX {
                if (nkn as usize) > k {
                    let c = (kosten((x, y), (nx, ny)) * SKALA) as i64 + 1;
                    g.kante(k, nkn as usize, c, c);
                }
            } else {
                let l = label[ny * lw + nx];
                if l == li {
                    g.endknoten(k, GROSS, 0);
                } else if l == lj {
                    g.endknoten(k, 0, GROSS);
                }
            }
        }
    }
    // Wie die Referenzumsetzung: Knoten im Senkenbaum gehen an j, alle
    // anderen (Quellbaum und frei) an i.
    let senkenseite = g.schneiden();
    for (k, &(x, y)) in lage.iter().enumerate() {
        label[y * lw + x] = if senkenseite[k] { lj } else { li };
    }
}

/// Minimaler Schnitt / maximaler Fluss nach Boykov & Kolmogorov, "An
/// Experimental Comparison of Min-Cut/Max-Flow Algorithms for Energy
/// Minimization in Vision" (PAMI 2004) — das Verfahren, das auch die
/// Referenzumsetzung von Brown & Lowe fuer den Nahtschnitt nutzt (`GCGraph`):
/// zwei Suchbaeume (Quelle, Senke) wachsen, werden nach jeder Augmentierung
/// repariert statt neu aufgebaut — auf Bildgittern um Groessenordnungen
/// schneller als Verfahren mit Neuaufbau je Phase.
/// Kanten paarweise (e, e ^ 1); Index 0/1 ist ein Platzhalter (0 = "keine").
/// Endknoten-Kapazitaet je Knoten: > 0 von der Quelle, < 0 zur Senke.
pub(super) struct Graph {
    /// Wert des Flusses = Kosten des minimalen Schnitts (nach `schneiden`).
    pub fluss: i64,
    erste: Vec<u32>,
    ende: Vec<i64>,
    ziel: Vec<u32>,
    naechste: Vec<u32>,
    kap: Vec<i64>,
}

const TERMINAL: i64 = -1;
const WAISE: i64 = -2;

impl Graph {
    pub(super) fn neu(n: usize) -> Self {
        Graph { fluss: 0, erste: vec![0; n], ende: vec![0; n], ziel: vec![0, 0], naechste: vec![0, 0], kap: vec![0, 0] }
    }
    /// Kante a -> b (Kapazitaet c_ab) und b -> a (c_ba).
    pub(super) fn kante(&mut self, a: usize, b: usize, c_ab: i64, c_ba: i64) {
        for (von, nach, c) in [(a, b, c_ab), (b, a, c_ba)] {
            self.ziel.push(nach as u32);
            self.kap.push(c);
            self.naechste.push(self.erste[von]);
            self.erste[von] = (self.ziel.len() - 1) as u32;
        }
    }
    /// Endknoten-Kapazitaeten (Quelle -> v, v -> Senke).
    pub(super) fn endknoten(&mut self, v: usize, quelle: i64, senke: i64) {
        let d = self.ende[v];
        let (q, s) = if d > 0 { (quelle + d, senke) } else { (quelle, senke - d) };
        // Beide Seiten zugleich: der kleinere Teil fliesst sofort.
        self.fluss += q.min(s);
        self.ende[v] = q - s;
    }

    /// Maximaler Fluss; danach liefert `quellseite` die Seite jedes Knotens.
    /// Rueckgabe: Baumzugehoerigkeit je Knoten (false = Quellbaum oder frei).
    pub(super) fn schneiden(&mut self) -> Vec<bool> {
        let n = self.erste.len();
        let mut eltern = vec![0i64; n];
        let mut baum = vec![false; n]; // true = Senkenbaum
        let mut zeit = vec![0u32; n];
        let mut tiefe = vec![0u32; n];
        let mut in_liste = vec![false; n];
        let mut aktiv = std::collections::VecDeque::new();
        for v in 0..n {
            if self.ende[v] != 0 {
                aktiv.push_back(v);
                in_liste[v] = true;
                tiefe[v] = 1;
                eltern[v] = TERMINAL;
                baum[v] = self.ende[v] < 0;
            }
        }
        let mut waisen: Vec<usize> = Vec::new();
        let mut jetzt = 0u32;
        loop {
            // Baeume wachsen lassen, bis eine Kante sie verbindet.
            let mut e0 = 0usize;
            while let Some(&v) = aktiv.front() {
                if eltern[v] != 0 {
                    let vt = baum[v] as usize;
                    let mut ei = self.erste[v] as usize;
                    while ei != 0 {
                        if self.kap[ei ^ vt] != 0 {
                            let u = self.ziel[ei] as usize;
                            if eltern[u] == 0 {
                                baum[u] = baum[v];
                                eltern[u] = (ei ^ 1) as i64;
                                zeit[u] = zeit[v];
                                tiefe[u] = tiefe[v] + 1;
                                if !in_liste[u] {
                                    in_liste[u] = true;
                                    aktiv.push_back(u);
                                }
                            } else if baum[u] != baum[v] {
                                e0 = ei ^ vt;
                                break;
                            } else if tiefe[u] > tiefe[v] + 1 && zeit[u] <= zeit[v] {
                                eltern[u] = (ei ^ 1) as i64;
                                zeit[u] = zeit[v];
                                tiefe[u] = tiefe[v] + 1;
                            }
                        }
                        ei = self.naechste[ei] as usize;
                    }
                    if e0 > 0 {
                        break;
                    }
                }
                aktiv.pop_front();
                in_liste[v] = false;
            }
            if e0 == 0 {
                break;
            }
            // Kleinste Restkapazitaet entlang des Pfads.
            let mut min = self.kap[e0];
            for k in [1usize, 0] {
                let mut v = self.ziel[e0 ^ k] as usize;
                loop {
                    let ei = eltern[v];
                    if ei < 0 {
                        break;
                    }
                    min = min.min(self.kap[ei as usize ^ k]);
                    v = self.ziel[ei as usize] as usize;
                }
                min = min.min(self.ende[v].abs());
            }
            // Augmentieren, Waisen sammeln.
            self.kap[e0] -= min;
            self.kap[e0 ^ 1] += min;
            self.fluss += min;
            for k in [1usize, 0] {
                let mut v = self.ziel[e0 ^ k] as usize;
                loop {
                    let ei = eltern[v];
                    if ei < 0 {
                        break;
                    }
                    let ei = ei as usize;
                    self.kap[ei ^ (k ^ 1)] += min;
                    self.kap[ei ^ k] -= min;
                    if self.kap[ei ^ k] == 0 {
                        waisen.push(v);
                        eltern[v] = WAISE;
                    }
                    v = self.ziel[ei] as usize;
                }
                self.ende[v] += if k == 1 { -min } else { min };
                if self.ende[v] == 0 {
                    waisen.push(v);
                    eltern[v] = WAISE;
                }
            }
            // Baeume reparieren: neue Eltern fuer die Waisen.
            jetzt += 1;
            while let Some(v) = waisen.pop() {
                let vt = baum[v] as usize;
                let mut beste = (u32::MAX, 0usize);
                let mut ei = self.erste[v] as usize;
                while ei != 0 {
                    if self.kap[ei ^ (vt ^ 1)] != 0 {
                        let mut u = self.ziel[ei] as usize;
                        if baum[u] as usize == vt && eltern[u] != 0 {
                            // Entfernung zur Wurzel
                            let mut d = 0u32;
                            loop {
                                if zeit[u] == jetzt {
                                    d += tiefe[u];
                                    break;
                                }
                                let ej = eltern[u];
                                d += 1;
                                if ej < 0 {
                                    if ej == WAISE {
                                        d = u32::MAX - 1;
                                    } else {
                                        zeit[u] = jetzt;
                                        tiefe[u] = 1;
                                    }
                                    break;
                                }
                                u = self.ziel[ej as usize] as usize;
                            }
                            if d < u32::MAX - 1 {
                                let mut d = d + 1;
                                if d < beste.0 {
                                    beste = (d, ei);
                                }
                                let mut u = self.ziel[ei] as usize;
                                while zeit[u] != jetzt {
                                    zeit[u] = jetzt;
                                    d -= 1;
                                    tiefe[u] = d;
                                    u = self.ziel[eltern[u] as usize] as usize;
                                }
                            }
                        }
                    }
                    ei = self.naechste[ei] as usize;
                }
                if beste.1 > 0 {
                    eltern[v] = beste.1 as i64;
                    zeit[v] = jetzt;
                    tiefe[v] = beste.0;
                    continue;
                }
                // Kein Elternteil: Knoten wird frei, Nachbarn neu pruefen.
                zeit[v] = 0;
                eltern[v] = 0;
                let mut ei = self.erste[v] as usize;
                while ei != 0 {
                    let u = self.ziel[ei] as usize;
                    let ej = eltern[u];
                    if baum[u] as usize == vt && ej != 0 {
                        if self.kap[ei ^ (vt ^ 1)] != 0 && !in_liste[u] {
                            in_liste[u] = true;
                            aktiv.push_back(u);
                        }
                        if ej > 0 && self.ziel[ej as usize] as usize == v {
                            waisen.push(u);
                            eltern[u] = WAISE;
                        }
                    }
                    ei = self.naechste[ei] as usize;
                }
            }
        }
        baum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schnitt_klein() {
        // Kette Quelle - 0 - 1 - 2 - 3 - Senke mit Kosten 5, 1, 4: der
        // minimale Schnitt trennt zwischen 1 und 2 (Kosten 1).
        let mut g = Graph::neu(4);
        g.endknoten(0, 1 << 40, 0);
        g.endknoten(3, 0, 1 << 40);
        g.kante(0, 1, 5, 5);
        g.kante(1, 2, 1, 1);
        g.kante(2, 3, 4, 4);
        let senke = g.schneiden();
        assert_eq!(senke, vec![false, false, true, true]);
        // Gitter 3x3, linke Spalte Quelle, rechte Senke, mittlere Spalte mit
        // billigen Kanten nach rechts: Schnitt zwischen Mitte und rechts.
        let mut g = Graph::neu(9);
        for y in 0..3 {
            g.endknoten(y * 3, 1 << 40, 0);
            g.endknoten(y * 3 + 2, 0, 1 << 40);
            g.kante(y * 3, y * 3 + 1, 10, 10);
            g.kante(y * 3 + 1, y * 3 + 2, 2, 2);
            if y < 2 {
                for x in 0..3 {
                    g.kante(y * 3 + x, y * 3 + 3 + x, 7, 7);
                }
            }
        }
        let senke = g.schneiden();
        for y in 0..3 {
            assert_eq!([senke[y * 3], senke[y * 3 + 1], senke[y * 3 + 2]], [false, false, true]);
        }
    }
}
