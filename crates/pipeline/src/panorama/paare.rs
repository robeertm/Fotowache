//! Paarpruefung: passen zwei Bilder zusammen, und mit welchen Punkten?
//!
//! Nach Brown & Lowe (IJCV 2007, Abschnitt 3): eindeutige Zuordnungen der
//! Merkmale, Homographie per RANSAC (`geometrie`), Annahme nach Gl. 13 auf den
//! Zuordnungen in der Ueberlappung.
//!
//! Danach die **gefuehrte Zuordnung** (Hartley & Zisserman, "Multiple View
//! Geometry", Alg. 4.6, "guided matching"): mit der Homographie wird das Feld um
//! jeden Merkmalspunkt samt oertlicher Verzerrung (Massstab, Perspektive) ins
//! andere Bild uebertragen und dort im kleinen Fenster per normierter
//! Kreuzkorrelation gesucht. Sie liefert mehr und genauere Punktpaare fuer den
//! Buendelausgleich als die Rohzuordnungen; ueber die Annahme entscheidet sie
//! nicht.
//!
//! **Rueckfallweg** fuer Bilder, die danach mit nichts verbunden sind
//! (Mehrfachkandidaten ohne Eindeutigkeitspruefung, Zwei-Punkt-Drehmodell bzw.
//! Aehnlichkeit, direkte Ausrichtung, siehe `abschliessen_rueckfall`). Gemessen:
//! mit der Annahme nur nach Gl. 13 rettet er kein Bild; mit der zusaetzlichen
//! Annahme ueber die direkte Ausrichtung behaelt er eine Reihe aus zwei
//! Aufnahmen vom fahrenden Schiff (starke Parallaxe), die sonst ganz verloren
//! ginge — den Restversatz dort behandelt die Parallaxe (Zhang & Liu).

use super::ausrichtung::Paar;
use super::geometrie::{self, abbilden, P2};
use super::mathe::{kreuz, mal_vek, normiert, skalar, Mat3, Zufall};
use super::merkmale::{self, Grau, Merkmale};

/// Ein Bild aus Sicht der Paarpruefung (Merkmalsmassstab, zentrierte Koordinaten).
pub struct Seite<'a> {
    pub m: &'a Merkmale,
    /// Graustufenkopie (Merkmalsmassstab) fuer die gefuehrte Suche.
    pub grau: &'a Grau,
    /// Hauptpunkt in Pixeln der Merkmalskopie.
    pub mitte: (f64, f64),
    /// Ausdehnung der Merkmalskopie.
    pub w: f64,
    pub h: f64,
}

impl Seite<'_> {
    fn punkt(&self, k: usize) -> P2 {
        (self.m.punkte[k].x as f64 - self.mitte.0, self.m.punkte[k].y as f64 - self.mitte.1)
    }
    /// Liegt der (zentrierte) Punkt im Bild?
    fn innen(&self, p: P2) -> bool {
        let (x, y) = (p.0 + self.mitte.0, p.1 + self.mitte.1);
        x >= 0.0 && y >= 0.0 && x <= self.w && y <= self.h
    }
}

/// Eine Aufloesungsstufe fuer die gefuehrte Suche: Graubild, Hauptpunkt,
/// Ausdehnung und die Merkmalsorte (zentriert, in Pixeln dieser Stufe).
struct Stufe<'a> {
    grau: &'a Grau,
    mitte: (f64, f64),
    w: f64,
    h: f64,
    punkte: Vec<P2>,
}

impl Stufe<'_> {
    fn innen(&self, p: P2, rand: f64) -> bool {
        let (x, y) = (p.0 + self.mitte.0, p.1 + self.mitte.1);
        x >= -rand && y >= -rand && x <= self.w + rand && y <= self.h + rand
    }
    /// Stufe im Massstab `s` (1 = Merkmalskopie) mit passendem Graubild.
    fn von<'b>(a: &Seite, grau: &'b Grau, s: f64) -> Stufe<'b> {
        Stufe {
            grau,
            mitte: (a.mitte.0 * s, a.mitte.1 * s),
            w: grau.w as f64,
            h: grau.h as f64,
            punkte: (0..a.m.punkte.len()).map(|k| a.punkt(k)).map(|p| (p.0 * s, p.1 * s)).collect(),
        }
    }
}

/// Ergebnis der gefuehrten Suche auf einer Stufe: Punktpaare, Indizes der
/// stimmigen, Zahl der geprueften Punkte, Homographie.
type Stufenergebnis = (Vec<(P2, P2)>, Vec<usize>, usize, Mat3);

/// Ergebnis einer Paarpruefung (auch fuer die Messausgabe).
pub struct Pruefung {
    pub zuordnungen: usize,
    pub inlier: usize,
    /// Merkmale in der Ueberlappung (kleinere Seite).
    pub ueberlappung: usize,
    pub paar: Option<Paar>,
}

const PUNKTE_JE_PAAR: usize = 250;
/// B&L Gl. 13.
const ALPHA: f64 = 8.0;
const BETA: f64 = 0.3;

#[inline]
fn grau_bei(g: &Grau, x: f64, y: f64) -> f32 {
    let fx = (x - 0.5).clamp(0.0, (g.w - 1) as f64);
    let fy = (y - 0.5).clamp(0.0, (g.h - 1) as f64);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(g.w - 1), (y0 + 1).min(g.h - 1));
    let (ax, ay) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let a = g.d[y0 * g.w + x0] * (1.0 - ax) + g.d[y0 * g.w + x1] * ax;
    let b = g.d[y1 * g.w + x0] * (1.0 - ax) + g.d[y1 * g.w + x1] * ax;
    a + (b - a) * ay
}

/// Feldradius der gefuehrten Suche (Merkmalsmassstab).
const FELD: isize = 7;

/// Punkte aus `a`, deren Feld (samt Suchrand) nach `h` ganz in `b` liegt —
/// ausgeduennt (mehrere Stufen liefern oft fast denselben Ort) und auf
/// hoechstens `max_punkte` gleichmaessig begrenzt.
fn suchpunkte(a: &Stufe, b: &Stufe, h: &Mat3, radius: f64, max_punkte: usize) -> Vec<P2> {
    let mut belegt = std::collections::HashSet::new();
    let mut kandidaten: Vec<P2> = Vec::new();
    let rand = FELD as f64 * 2.0 + radius + 3.0;
    for &p in &a.punkte {
        let schluessel = (((p.0 + a.mitte.0) / 3.0) as i64, ((p.1 + a.mitte.1) / 3.0) as i64);
        if !belegt.insert(schluessel) {
            continue;
        }
        let q = abbilden(h, p);
        if !q.0.is_finite() || !b.innen(q, -rand) || !a.innen(p, -(FELD as f64 + 2.0)) {
            continue;
        }
        kandidaten.push(p);
    }
    if kandidaten.len() > max_punkte {
        let schritt = kandidaten.len() as f64 / max_punkte as f64;
        kandidaten = (0..max_punkte).map(|t| kandidaten[(t as f64 * schritt) as usize]).collect();
    }
    kandidaten
}

/// Vorlage um `p` in `a`: Werte, Mittel, Streuung (Summe). `None` = strukturlos.
fn vorlage(a: &Stufe, p: P2) -> Option<(Vec<f32>, f32, f32)> {
    let (ax, ay) = (p.0 + a.mitte.0, p.1 + a.mitte.1);
    let mut t = Vec::with_capacity(((2 * FELD + 1) * (2 * FELD + 1)) as usize);
    for oy in -FELD..=FELD {
        for ox in -FELD..=FELD {
            t.push(grau_bei(a.grau, ax + ox as f64, ay + oy as f64));
        }
    }
    let n = t.len() as f32;
    let mt = t.iter().sum::<f32>() / n;
    let st = t.iter().map(|x| (x - mt) * (x - mt)).sum::<f32>().sqrt();
    (st / n.sqrt() >= 0.01).then_some((t, mt, st))
}

/// Korrelation der Vorlage mit `b` an der Stelle `q` + Versatz, Feld mit der
/// oertlichen Verzerrung `j` uebertragen.
#[allow(clippy::too_many_arguments)]
fn korrelation(b: &Stufe, q: P2, j: &[[f64; 2]; 2], dx: f64, dy: f64, t: &[f32], mt: f32, st: f32, v: &mut [f32]) -> f32 {
    let (bx, by) = (q.0 + b.mitte.0, q.1 + b.mitte.1);
    let mut k = 0;
    for oy in -FELD..=FELD {
        for ox in -FELD..=FELD {
            let (o0, o1) = (ox as f64, oy as f64);
            v[k] = grau_bei(b.grau, bx + j[0][0] * o0 + j[0][1] * o1 + dx, by + j[1][0] * o0 + j[1][1] * o1 + dy);
            k += 1;
        }
    }
    let n = v.len() as f32;
    let mv = v.iter().sum::<f32>() / n;
    let (mut sv, mut stv) = (0.0f32, 0.0f32);
    for k in 0..v.len() {
        let d = v[k] - mv;
        sv += d * d;
        stv += d * (t[k] - mt);
    }
    stv / (sv.sqrt() * st).max(1e-12)
}

fn oertlich(h: &Mat3, p: P2) -> (P2, [[f64; 2]; 2]) {
    let q = abbilden(h, p);
    let qx = abbilden(h, (p.0 + 1.0, p.1));
    let qy = abbilden(h, (p.0, p.1 + 1.0));
    (q, [[qx.0 - q.0, qy.0 - q.0], [qx.1 - q.1, qy.1 - q.1]])
}

/// Gefuehrte Zuordnung: fuer Punkte aus `a` (hoechstens `max_punkte`,
/// gleichmaessig ausgewaehlt) das Feld samt oertlicher Verzerrung von `h` nach
/// `b` uebertragen und im Umkreis `radius` per normierter Kreuzkorrelation
/// suchen. Rueckgabe: Punktpaare (zentriert) und Zahl der geprueften Punkte
/// mit genug Struktur (fuer die Annahmeregel).
fn gefuehrt(a: &Stufe, b: &Stufe, h: &Mat3, radius: f64, max_punkte: usize) -> (Vec<(P2, P2)>, usize) {
    let kandidaten = suchpunkte(a, b, h, radius, max_punkte);
    let r = radius.ceil() as isize;
    let mut geprueft = 0usize;
    let mut aus = Vec::new();
    let mut gruende = [0usize; 4]; // mehrdeutig, schwach, Rand, Summe NCC*100
    let mut v = vec![0.0f32; ((2 * FELD + 1) * (2 * FELD + 1)) as usize];
    for p in kandidaten {
        let Some((t, mt, st)) = vorlage(a, p) else { continue }; // strukturlos (Himmel, Wasser)
        geprueft += 1;
        let (q, j) = oertlich(h, p);
        let mut ncc = |dx: f64, dy: f64| korrelation(b, q, &j, dx, dy, &t, mt, st, &mut v);
        // Korrelation im Suchfenster; gewaehlt wird der Gipfel, der der
        // Vorhersage am naechsten liegt (gefuehrte Zuordnung: die Geometrie
        // entscheidet, Hartley & Zisserman Alg. 4.6). Zwei fast gleich nahe
        // Gipfel: mehrdeutig, verworfen.
        let seite = (2 * r + 1) as usize;
        let mut karte = vec![0.0f32; seite * seite];
        for iy in 0..seite {
            for ix in 0..seite {
                karte[iy * seite + ix] = ncc((ix as isize - r) as f64, (iy as isize - r) as f64);
            }
        }
        let mut gipfel: Vec<(f64, f32, isize, isize)> = Vec::new();
        let mut best_wert = f32::MIN;
        for iy in 0..seite {
            for ix in 0..seite {
                let c = karte[iy * seite + ix];
                best_wert = best_wert.max(c);
                if c < 0.75 {
                    continue;
                }
                let mut hoechster = true;
                for (ny, nx) in [(iy.wrapping_sub(1), ix), (iy + 1, ix), (iy, ix.wrapping_sub(1)), (iy, ix + 1)] {
                    if ny < seite && nx < seite && karte[ny * seite + nx] > c {
                        hoechster = false;
                    }
                }
                if hoechster {
                    let (dx, dy) = (ix as isize - r, iy as isize - r);
                    gipfel.push((((dx * dx + dy * dy) as f64).sqrt(), c, dx, dy));
                }
            }
        }
        gruende[3] += (best_wert.max(0.0) * 100.0) as usize;
        gipfel.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let Some(&(abst, best, bx0, by0)) = gipfel.first() else {
            gruende[1] += 1;
            continue;
        };
        if gipfel.get(1).is_some_and(|z| z.0 - abst < 1.0) {
            gruende[0] += 1;
            continue;
        }
        if bx0.abs() >= r || by0.abs() >= r {
            gruende[2] += 1;
            continue;
        }
        // Unterpixel per Parabel
        let parabel = |m: f32, l: f32, rr: f32| {
            let n = l - 2.0 * m + rr;
            if n.abs() < 1e-9 { 0.0 } else { (0.5 * (l - rr) / n).clamp(-0.5, 0.5) as f64 }
        };
        let (fx, fy) = (bx0 as f64, by0 as f64);
        let sx = parabel(best, ncc(fx - 1.0, fy), ncc(fx + 1.0, fy));
        let sy = parabel(best, ncc(fx, fy - 1.0), ncc(fx, fy + 1.0));
        aus.push((p, (q.0 + fx + sx, q.1 + fy + sy)));
    }
    super::diagnose(&format!(
        "    Suche r {radius}: mehrdeutig {}, schwach {}, am Rand {}, mittlere Korrelation {:.2}",
        gruende[0], gruende[1], gruende[2], gruende[3] as f64 / 100.0 / geprueft.max(1) as f64
    ));
    (aus, geprueft)
}

/// Gefuehrte Zuordnung fuer ein angenommenes Paar (Homographie `h0`, a -> b,
/// zentriert): erst weit mit wenigen Punkten, dann eng mit allen, robust
/// nachschaetzen. `vorab` = (n_i, n_f) der Annahme nach Gl. 13.
fn abschliessen(i: usize, j: usize, a: &Seite, b: &Seite, h0: Mat3, zuordnungen: usize, vorab: (usize, usize)) -> Pruefung {
    let leer = |inlier| Pruefung { zuordnungen, inlier, ueberlappung: 0, paar: None };
    let Some((z, innen, _, h)) = gefuehrte_zuordnung(i, j, a, b, h0) else { return leer(0) };
    let (vi, vf) = vorab;
    let n_i = innen.len();
    if n_i < 4 {
        return leer(vi);
    }
    let schritt = (n_i as f64 / PUNKTE_JE_PAAR as f64).max(1.0);
    let punkte: Vec<[f64; 4]> = (0..n_i.min(PUNKTE_JE_PAAR))
        .map(|t| {
            let (p, q) = z[innen[(t as f64 * schritt) as usize]];
            [p.0, p.1, q.0, q.1]
        })
        .collect();
    Pruefung { zuordnungen, inlier: vi, ueberlappung: vf, paar: Some(Paar { i, j, h, punkte, inlier: vi }) }
}

/// Rueckfallweg (nur fuer Bilder, die nach dem Standardweg mit nichts verbunden
/// sind): direkte Ausrichtung der Hypothese `h0` (`direkt`: Szeliski 2006,
/// "Image Alignment and Stitching", Abschnitt 3 — hierarchische direkte
/// Ausrichtung auf den Pixeln), dann gefuehrte Zuordnung. Angenommen wird nach
/// Gl. 13 auf den gefuehrten Punkten ODER, bei Parallaxe (dort passt nur ein
/// Teil der Ueberlappung zu EINER Homographie), wenn die direkte Ausrichtung
/// die Ueberlappung deutlich bestaetigt: Korrelation der strukturierten Pixel
/// mindestens 0,5 und mindestens 15 stimmige Punkte. Die Schwelle 0,5 ist gemessen
/// (Pruefstand der Vorversion): fremde Bilder kamen nicht ueber 0,3, echte
/// Paare mit Parallaxe auf 0,57 bis 0,91.
fn abschliessen_rueckfall(i: usize, j: usize, a: &Seite, b: &Seite, h0: Mat3, zuordnungen: usize) -> Pruefung {
    let leer = |inlier| Pruefung { zuordnungen, inlier, ueberlappung: 0, paar: None };
    let Some(d) = super::direkt::ausrichten(a.grau, a.mitte, b.grau, b.mitte, &h0) else { return leer(0) };
    super::diagnose(&format!("  direkt {}-{}: Korrelation {:.2}, Ueberlappung {:.0} %", i + 1, j + 1, d.korrelation, d.anteil * 100.0));
    if d.korrelation <= 0.3 || d.anteil <= 0.02 {
        return leer(0);
    }
    let Some((z, innen, n_f, h)) = gefuehrte_zuordnung(i, j, a, b, d.h) else { return leer(0) };
    let n_i = innen.len();
    let ok = n_i >= 15 && (n_i as f64 > ALPHA + BETA * n_f as f64 || d.korrelation >= 0.5);
    super::diagnose(&format!("  Annahme {}-{} (Rueckfall): {n_i} stimmig, {n_f} geprueft, Korrelation {:.2} -> {}", i + 1, j + 1, d.korrelation, if ok { "ja" } else { "nein" }));
    if !ok {
        return Pruefung { zuordnungen, inlier: n_i, ueberlappung: n_f, paar: None };
    }
    let schritt = (n_i as f64 / PUNKTE_JE_PAAR as f64).max(1.0);
    let punkte: Vec<[f64; 4]> = (0..n_i.min(PUNKTE_JE_PAAR))
        .map(|t| {
            let (p, q) = z[innen[(t as f64 * schritt) as usize]];
            [p.0, p.1, q.0, q.1]
        })
        .collect();
    Pruefung { zuordnungen, inlier: n_i, ueberlappung: n_f, paar: Some(Paar { i, j, h, punkte, inlier: n_i }) }
}

/// Gefuehrte Suche mit der Hypothese `h` (a -> b, zentriert). Rueckgabe:
/// Punktpaare (Merkmalsmassstab), Indizes der stimmigen, Zahl der geprueften
/// Punkte mit Struktur, nachgeschaetzte Homographie.
fn gefuehrte_zuordnung(i: usize, j: usize, a: &Seite, b: &Seite, h: Mat3) -> Option<Stufenergebnis> {
    // Gefuehrte Suche auf zwei Stufen: Merkmalskopie und halbe Aufloesung. Wo
    // ein Bild perspektivisch gestreckt ist und die Struktur fein (Fenster von
    // drei, vier Pixeln), korrelieren die Felder erst auf der halben Stufe —
    // gemessen: doppelt so viele Treffer. Genommen wird die Stufe, die das
    // Paar am deutlichsten bestaetigt.
    let halb = std::cell::OnceCell::new();
    let halb = || halb.get_or_init(|| (super::direkt::halbieren(a.grau), super::direkt::halbieren(b.grau)));
    let mut ergebnis: Option<Stufenergebnis> = None;
    for s in [1.0, 0.5] {
        // Volle Stufe bestaetigt das Paar schon deutlich: halbe sparen.
        if s < 1.0 && ergebnis.as_ref().is_some_and(|e| e.1.len() as f64 > 2.0 * (ALPHA + BETA * e.2 as f64)) {
            break;
        }
        let (ga, gb) = if s == 1.0 { (a.grau, b.grau) } else { (&halb().0, &halb().1) };
        let (sa, sb) = (Stufe::von(a, ga, s), Stufe::von(b, gb, s));
        // h auf die Stufe umrechnen: S h S^-1
        let mut hs = [[h[0][0], h[0][1], h[0][2] * s], [h[1][0], h[1][1], h[1][2] * s], [h[2][0] / s, h[2][1] / s, h[2][2]]];
        let mut stufe_erg = None;
        for (r, max) in [(4.0, 800usize), (2.0, 2500)] {
            let (z, geprueft) = gefuehrt(&sa, &sb, &hs, r, max);
            let p: Vec<P2> = z.iter().map(|v| v.0).collect();
            let q: Vec<P2> = z.iter().map(|v| v.1).collect();
            let Some(e) = geometrie::ransac(&p, &q, geometrie::RANSAC_TOLERANZ, (i * 31 + j) as u64 + 7) else {
                super::diagnose(&format!("  gefuehrt {}-{} Stufe {s} Radius {r}: {geprueft} geprueft, {} gefunden, keine Geometrie", i + 1, j + 1, z.len()));
                stufe_erg = None;
                break;
            };
            super::diagnose(&format!("  gefuehrt {}-{} Stufe {s} Radius {r}: {geprueft} geprueft, {} gefunden, {} stimmig", i + 1, j + 1, z.len(), e.innen.len()));
            hs = e.h;
            stufe_erg = Some((z, e.innen, geprueft));
        }
        let Some((z, innen, geprueft)) = stufe_erg else { continue };
        let guete = |n_i: usize, n_f: usize| n_i as f64 / (ALPHA + BETA * n_f as f64);
        if ergebnis.as_ref().is_none_or(|e| guete(innen.len(), geprueft) > guete(e.1.len(), e.2)) {
            // zurueck in Koordinaten der Merkmalskopie
            let z = z.into_iter().map(|(p, q)| ((p.0 / s, p.1 / s), (q.0 / s, q.1 / s))).collect();
            let hv = [[hs[0][0], hs[0][1], hs[0][2] / s], [hs[1][0], hs[1][1], hs[1][2] / s], [hs[2][0] * s, hs[2][1] * s, hs[2][2]]];
            ergebnis = Some((z, innen, geprueft, hv));
        }
    }
    ergebnis
}

/// Standardweg: eindeutige Zuordnungen `z` (aus `merkmale::zuordnen`) +
/// Homographie per RANSAC, danach gefuehrte Zuordnung (Hartley & Zisserman,
/// "Multiple View Geometry", Alg. 4.6, Schritt "guided matching": mit der
/// geschaetzten Homographie weitere Zuordnungen im Umkreis der uebertragenen
/// Punkte suchen) und Annahme nach B&L Gl. 13.
pub fn standard(i: usize, j: usize, a: &Seite, b: &Seite, z: &[(usize, usize)]) -> Pruefung {
    let p: Vec<P2> = z.iter().map(|&(x, _)| a.punkt(x)).collect();
    let q: Vec<P2> = z.iter().map(|&(_, y)| b.punkt(y)).collect();
    let Some(e) = geometrie::ransac(&p, &q, geometrie::RANSAC_TOLERANZ, (i * 7919 + j) as u64 + 1) else {
        return Pruefung { zuordnungen: z.len(), inlier: 0, ueberlappung: 0, paar: None };
    };
    // Annahme nach B&L Gl. 13: n_i > α + β n_f (α = 8, β = 0,3; aus dem
    // Binomialmodell Gl. 7–12 mit p1 = 0,6, p0 = 0,1, p(m = 1) = 1e-6,
    // p_min = 0,999). n_i = RANSAC-Innenpunkte, n_f = Zuordnungen, deren Punkt
    // in der Ueberlappung liegt ("features inside the area of overlap",
    // Abschnitt 3.2: Innen- plus Ausreisser dort). Die Referenzumsetzung zaehlt
    // vereinfachend alle Zuordnungen des Paars.
    let n_i = e.innen.len();
    let n_f = p.iter().filter(|&&x| {
        let y = abbilden(&e.h, x);
        y.0.is_finite() && b.innen(y)
    }).count();
    let ok = n_i as f64 > ALPHA + BETA * n_f as f64;
    super::diagnose(&format!("  Annahme {}-{} (B&L Gl. 13): {n_i} Innenpunkte, {n_f} Zuordnungen in der Ueberlappung -> {}", i + 1, j + 1, if ok { "ja" } else { "nein" }));
    if ok {
        let mit = abschliessen(i, j, a, b, e.h, z.len(), (n_i, n_f));
        if mit.paar.is_some() {
            return mit;
        }
        // Gefuehrte Zuordnung ohne Ergebnis: die RANSAC-Innenpunkte selbst.
        let punkte: Vec<[f64; 4]> = e.innen.iter().map(|&k| [p[k].0, p[k].1, q[k].0, q[k].1]).collect();
        return Pruefung { zuordnungen: z.len(), inlier: n_i, ueberlappung: n_f, paar: Some(Paar { i, j, h: e.h, punkte, inlier: n_i }) };
    }
    Pruefung { zuordnungen: z.len(), inlier: n_i, ueberlappung: n_f, paar: None }
}

/// Rueckfallweg: Mehrfachkandidaten + Zwei-Punkt-RANSAC ueber das Drehmodell.
/// `brennweiten`: zu probierende Brennweiten (Merkmalsmassstab).
pub fn rueckfall(i: usize, j: usize, a: &Seite, b: &Seite, brennweiten: &[f64], bekannt: bool) -> Pruefung {
    let kand = merkmale::kandidaten(a.m, b.m, 2, 96);
    let p: Vec<P2> = kand.iter().map(|k| a.punkt(k.0)).collect();
    let q: Vec<P2> = kand.iter().map(|k| b.punkt(k.1)).collect();
    let dim = a.w.max(a.h).max(b.w).max(b.h);
    // Bei geratener Brennweite passt das Drehmodell nur ungefaehr — grosszuegig
    // zaehlen, die gefuehrte Zuordnung macht es danach genau.
    let schwelle = if bekannt { 4.0 } else { (0.008 * dim).max(4.0) };
    let mut beste: Option<(usize, Mat3, f64)> = None;
    for (n, &f) in brennweiten.iter().enumerate() {
        if let Some((r, innen)) = drehung_ransac(&p, &q, &kand, f, schwelle, (i * 131 + j * 17 + n) as u64 + 3) {
            let w = mal_vek(&r, &[0.0, 0.0, f]);
            super::diagnose(&format!(
                "  Rueckfall {}-{}: f {:.0}: {} stimmige Kandidaten, Bildmitte -> ({:.0}, {:.0})",
                i + 1, j + 1, f, innen, f * w[0] / w[2], f * w[1] / w[2]
            ));
            if beste.as_ref().is_none_or(|b| innen > b.0) {
                beste = Some((innen, r, f));
            }
        }
    }
    let mut hypothesen: Vec<(&str, Mat3)> = Vec::new();
    if let Some((innen, r, f)) = beste {
        if innen >= 8 {
            // H = K R K^-1 fuer zentrierte Koordinaten
            hypothesen.push((
                "Drehung",
                [[r[0][0], r[0][1], r[0][2] * f], [r[1][0], r[1][1], r[1][2] * f], [r[2][0] / f, r[2][1] / f, r[2][2]]],
            ));
        }
    }
    // Aehnlichkeit (Massstab, Drehung, Verschiebung): fuer Aufnahmen, zwischen
    // denen der Standpunkt naeher ans Motiv gerueckt ist — dort erscheint alles
    // groesser, was keine reine Drehung erklaeren kann.
    if let Some((innen, h)) = aehnlichkeit_ransac(&p, &q, &kand, (0.01 * dim).max(4.0), (i * 37 + j) as u64 + 11) {
        super::diagnose(&format!("  Rueckfall {}-{}: Aehnlichkeit: {} stimmige Kandidaten, Massstab {:.2}", i + 1, j + 1, innen, (h[0][0] * h[1][1] - h[0][1] * h[1][0]).sqrt()));
        if innen >= 8 {
            hypothesen.push(("Aehnlichkeit", h));
        }
    }
    let mut ergebnis = Pruefung { zuordnungen: kand.len(), inlier: 0, ueberlappung: 0, paar: None };
    for (name, h) in hypothesen {
        let e = abschliessen_rueckfall(i, j, a, b, h, kand.len());
        super::diagnose(&format!("  Rueckfall {}-{} aus {name}: {} Inlier{}", i + 1, j + 1, e.inlier, if e.paar.is_some() { " (angenommen)" } else { "" }));
        let besser = match (&e.paar, &ergebnis.paar) {
            (Some(_), None) => true,
            (None, Some(_)) => false,
            _ => e.inlier > ergebnis.inlier,
        };
        if besser {
            ergebnis = e;
        }
    }
    ergebnis
}

/// RANSAC ueber eine Aehnlichkeitsabbildung aus je zwei Punktpaaren
/// (komplexe Zahlen: q = s * p + t). Massstab 0,5..2, Drehung bis 25 Grad.
fn aehnlichkeit_ransac(p: &[P2], q: &[P2], kand: &[(usize, usize, f32)], schwelle: f64, saat: u64) -> Option<(usize, Mat3)> {
    let n = p.len();
    if n < 8 {
        return None;
    }
    let mut ordnung: Vec<usize> = (0..n).collect();
    ordnung.sort_by(|&x, &y| kand[x].2.partial_cmp(&kand[y].2).unwrap());
    let oben = (n / 4).max(8).min(n);
    let s2 = schwelle * schwelle;
    let mut z = Zufall::neu(saat);
    let mut beste: Option<(usize, Mat3)> = None;
    let mut max_iter = 4000usize;
    let mut it = 0;
    let mut gezaehlt = vec![u32::MAX; kand.iter().map(|k| k.0).max().unwrap_or(0) + 1];
    while it < max_iter {
        it += 1;
        let pool = if it % 2 == 0 { oben } else { n };
        let (k1, k2) = (ordnung[z.bis(pool)], ordnung[z.bis(pool)]);
        if kand[k1].0 == kand[k2].0 || kand[k1].1 == kand[k2].1 {
            continue;
        }
        let (dpx, dpy) = (p[k1].0 - p[k2].0, p[k1].1 - p[k2].1);
        let (dqx, dqy) = (q[k1].0 - q[k2].0, q[k1].1 - q[k2].1);
        let nn = dpx * dpx + dpy * dpy;
        if nn < 400.0 {
            continue; // zu nah beieinander: Massstab unbestimmt
        }
        // s = dq / dp (komplex)
        let (sr, si) = ((dqx * dpx + dqy * dpy) / nn, (dqy * dpx - dqx * dpy) / nn);
        let m = (sr * sr + si * si).sqrt();
        if !(0.5..=2.0).contains(&m) || si.atan2(sr).abs() > 25f64.to_radians() {
            continue;
        }
        let tx = q[k1].0 - (sr * p[k1].0 - si * p[k1].1);
        let ty = q[k1].1 - (si * p[k1].0 + sr * p[k1].1);
        let mut c = 0usize;
        for k in 0..n {
            let x = sr * p[k].0 - si * p[k].1 + tx;
            let y = si * p[k].0 + sr * p[k].1 + ty;
            if (x - q[k].0).powi(2) + (y - q[k].1).powi(2) < s2 && gezaehlt[kand[k].0] != it as u32 {
                gezaehlt[kand[k].0] = it as u32;
                c += 1;
            }
        }
        if beste.as_ref().is_none_or(|b| c > b.0) {
            beste = Some((c, [[sr, -si, tx], [si, sr, ty], [0.0, 0.0, 1.0]]));
            let w = c as f64 / n as f64;
            let p2 = (w * w).clamp(1e-12, 1.0 - 1e-12);
            let noetig = ((0.001f64).ln() / (1.0 - p2).ln()).ceil();
            max_iter = (noetig.max(200.0) as usize).min(4000);
        }
    }
    beste
}

/// RANSAC ueber eine Drehung aus je zwei Punktpaaren (Dreibein-Verfahren).
/// Rueckgabe: Drehung (Strahlen a -> b) und Zahl der stimmigen Kandidaten
/// (jeder Punkt aus `a` hoechstens einmal gezaehlt).
fn drehung_ransac(p: &[P2], q: &[P2], kand: &[(usize, usize, f32)], f: f64, schwelle: f64, saat: u64) -> Option<(Mat3, usize)> {
    let n = p.len();
    if n < 8 {
        return None;
    }
    let ua: Vec<[f64; 3]> = p.iter().map(|v| normiert(&[v.0, v.1, f])).collect();
    let ub: Vec<[f64; 3]> = q.iter().map(|v| normiert(&[v.0, v.1, f])).collect();
    // Bessere Kandidaten zuerst ziehen (PROSAC-artig): die Haelfte der
    // Stichproben kommt aus dem besten Viertel.
    let mut ordnung: Vec<usize> = (0..n).collect();
    ordnung.sort_by(|&x, &y| kand[x].2.partial_cmp(&kand[y].2).unwrap());
    let oben = (n / 4).max(8).min(n);
    let s2 = schwelle * schwelle;
    let winkel_tol = 3.0 * schwelle / f;
    let mut z = Zufall::neu(saat);
    let mut beste: Option<(usize, Mat3)> = None;
    let mut max_iter = 4000usize;
    let mut it = 0;
    let mut gezaehlt = vec![u32::MAX; kand.iter().map(|k| k.0).max().unwrap_or(0) + 1];
    while it < max_iter {
        it += 1;
        let pool = if it % 2 == 0 { oben } else { n };
        let k1 = ordnung[z.bis(pool)];
        let k2 = ordnung[z.bis(pool)];
        if kand[k1].0 == kand[k2].0 || kand[k1].1 == kand[k2].1 {
            continue;
        }
        let (a1, a2, b1, b2) = (ua[k1], ua[k2], ub[k1], ub[k2]);
        // Gleicher Winkel zwischen den beiden Strahlen in beiden Bildern?
        let wa = skalar(&a1, &a2).clamp(-1.0, 1.0).acos();
        let wb = skalar(&b1, &b2).clamp(-1.0, 1.0).acos();
        if (wa - wb).abs() > winkel_tol || wa < 0.01 {
            continue;
        }
        let dreibein = |u: [f64; 3], v: [f64; 3]| {
            let e2 = normiert(&kreuz(&u, &v));
            let e3 = kreuz(&u, &e2);
            [u, e2, e3]
        };
        let ta = dreibein(a1, a2);
        let tb = dreibein(b1, b2);
        // R = Tb * Ta^T  (Spalten = Achsen)
        let mut r = [[0.0; 3]; 3];
        for x in 0..3 {
            for y in 0..3 {
                r[x][y] = (0..3).map(|k| tb[k][x] * ta[k][y]).sum();
            }
        }
        let mut c = 0usize;
        for k in 0..n {
            let w = mal_vek(&r, &ua[k]);
            if w[2] <= 1e-6 {
                continue;
            }
            let (x, y) = (f * w[0] / w[2], f * w[1] / w[2]);
            if (x - q[k].0).powi(2) + (y - q[k].1).powi(2) < s2 && gezaehlt[kand[k].0] != it as u32 {
                gezaehlt[kand[k].0] = it as u32;
                c += 1;
            }
        }
        if beste.as_ref().is_none_or(|b| c > b.0) {
            beste = Some((c, r));
            let w = c as f64 / n as f64;
            let p2 = (w * w).clamp(1e-12, 1.0 - 1e-12);
            let noetig = ((0.001f64).ln() / (1.0 - p2).ln()).ceil();
            max_iter = (noetig.max(200.0) as usize).min(4000);
        }
    }
    beste.map(|(c, r)| (r, c))
}

/// Brennweiten fuer den Rueckfallweg, wenn keine bekannt ist (Vielfache der
/// laengeren Bildseite: von Weitwinkel bis Tele).
pub fn brennweiten_raten(dim: f64) -> Vec<f64> {
    [0.7, 1.0, 1.4, 2.0, 3.0].iter().map(|k| k * dim).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panorama::mathe::{drehung, mul, transp};

    #[test]
    fn drehung_aus_zwei_punkten() {
        // 200 Kandidaten, davon nur 10 % richtig: die Zwei-Punkt-Schaetzung muss
        // die Drehung trotzdem finden.
        let f = 900.0;
        let r = drehung(&[0.02, 0.35, -0.01]);
        let mut z = Zufall::neu(5);
        let (mut p, mut q, mut kand) = (vec![], vec![], vec![]);
        for k in 0..200 {
            let a = ((z.gleich() - 0.5) * 1000.0, (z.gleich() - 0.5) * 700.0);
            let b = if k % 10 == 0 {
                let w = mal_vek(&r, &[a.0, a.1, f]);
                (f * w[0] / w[2], f * w[1] / w[2])
            } else {
                ((z.gleich() - 0.5) * 1000.0, (z.gleich() - 0.5) * 700.0)
            };
            p.push(a);
            q.push(b);
            kand.push((k, k, 1.0f32));
        }
        let (rs, innen) = drehung_ransac(&p, &q, &kand, f, 2.0, 1).expect("Drehung");
        assert!(innen >= 20, "{innen}");
        let d = mul(&transp(&rs), &r);
        assert!((d[0][0] + d[1][1] + d[2][2] - 3.0).abs() < 1e-4, "{d:?}");
    }
}
