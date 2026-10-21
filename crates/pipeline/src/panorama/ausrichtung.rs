//! Globale Ausrichtung: Buendelausgleich und Begradigung nach Brown & Lowe,
//! "Automatic Panoramic Image Stitching using Invariant Features", IJCV 2007
//! (im Folgenden B&L), Abschnitte 2, 4 und 5.
//!
//! ## Kameramodell (B&L Gl. 1–3)
//! Alle Aufnahmen vom selben Standpunkt, nur gedreht. Kamera k: Drehung R_k
//! (Kamera -> Welt), Brennweite f, Hauptpunkt in der Bildmitte. Ein Bildpunkt
//! (x, y) (zentriert) ist der Strahl R_k * (x, y, f). Zwischen zwei Bildern gilt
//! H_ij = K_i R_i^T R_j K_j^-1 (Gl. 1; die Arbeit schreibt R fuer Welt -> Kamera).
//!
//! **Brennweite:** B&L schaetzen eine Brennweite je Bild. Wo die Metadaten fuer
//! mehrere Bilder dieselbe Brennweite nennen, ist es dieselbe Optik im selben
//! Zustand — dann gibt es EINE gemeinsame Brennweite fuer diese Gruppe (strenger
//! als B&L, das Modell hat weniger Freiheit, Fehler zu schlucken). Ohne
//! Metadaten bleibt es bei B&L: eine je Bild. Einen Massstabsfaktor je Bild
//! gibt es nicht. Die Metadaten-Brennweite ist nur Startwert, geschaetzt wird
//! wie bei B&L: am Pruefstand (13 Reihen, Median des Nahtversatzes) war die
//! geschaetzte Brennweite in 7 Reihen besser, die feste in 4 (Reihe mit
//! Meereshorizont: 1,20 px geschaetzt gegen 1,80 px fest).
//!
//! **Verzeichnung:** nicht in B&L modelliert; die Arbeit nennt sie unter
//! "Future Work: Advanced Camera Modelling" ("high quality image stitching
//! applications would need to include radial distortion parameters at least in
//! the bundle adjustment and rendering stages", Abb. 9: x' = (1 + κ|x|²) x).
//! Ist sie per Objektivprofil schon herausgerechnet (`Eingabe::entzerrt`),
//! bleibt k1 = 0 fest. Sonst ein gemeinsames k1 je Optik (gleiche
//! Metadaten-Brennweite bzw. alle Bilder ohne Metadaten), r bezogen auf die
//! halbe Bilddiagonale.
//!
//! ## Buendelausgleich (B&L Abschnitt 4)
//! Bilder werden einzeln hinzugefuegt, das am besten passende (meiste stimmige
//! Punktpaare zu einem schon ausgerichteten Bild) zuerst; es startet mit
//! Drehung und Brennweite dieses Partners. (Gemessen an 2094–2098: ein Start
//! aus der Paar-Homographie wie in der Referenzumsetzung endet beim selben
//! Ergebnis, 28 px mittlerer Fehler — die Hoch- und Querformataufnahmen dort
//! stammen von verschiedenen Standpunkten, das Drehmodell kann das nicht
//! abbilden; die Referenzumsetzung scheitert dort ebenso.) Nach jedem Hinzufuegen
//! Levenberg-Marquardt ueber alle Parameter, Schritt nach Gl. 18
//! Φ = (JᵀJ + λ C_p⁻¹)⁻¹ Jᵀ r mit C_p = diag(σθ², …, σf², …), σθ = π/16,
//! σf = f̄/10 (Gl. 19; f̄ = Mittel der bisher geschaetzten Brennweiten). Fuer k1
//! (nicht in B&L) σk = 0,1: ein Schritt σk verschiebt die Bildecke um
//! 0,1 · halbe Diagonale — so weit wie ein Schritt σf = f/10 (gleiche
//! Pixelwirkung, also gleich "vorsichtig" gedaempft).
//! Fehlermass Gl. 14–17: jedes Punktpaar in beide Richtungen projiziert,
//! Huber-Funktion h(x) = |x|² (|x| < σ), 2σ|x| − σ² sonst; σ = ∞ waehrend des
//! Hinzufuegens, σ = 2 px fuer die Endloesung. λ wird je Schritt so gewaehlt,
//! dass die Kosten sinken (×10 / ÷10 wie in der Referenzumsetzung von Brown &
//! Lowe). Ableitungen als zentrale Differenzen (wie die Referenzumsetzung;
//! B&L leiten analytisch ab — das aendert die Loesung nicht, nur die Rechenzeit).
//! Die Drehung des ersten Bildes bleibt fest: das legt die freie Gesamtdrehung
//! fest (die Loesung ist dieselbe bis auf diese Drehung, die die Begradigung
//! ohnehin neu waehlt).

use super::mathe::*;

/// Ein bestaetigtes Bildpaar.
#[derive(Clone)]
pub struct Paar {
    pub i: usize,
    pub j: usize,
    /// Homographie i -> j in zentrierten Koordinaten (Merkmalsmassstab).
    pub h: Mat3,
    /// Innenpunkte [xi, yi, xj, yj], zentriert.
    pub punkte: Vec<[f64; 4]>,
    /// Zahl der stimmigen Punktpaare (n_i in B&L Gl. 13).
    pub inlier: usize,
}

/// Gruppennummer je Bild (zusammenhaengende Bestandteile ueber die Paare).
pub fn gruppen(n: usize, paare: &[Paar]) -> Vec<usize> {
    let mut e: Vec<usize> = (0..n).collect();
    fn w(e: &mut [usize], mut x: usize) -> usize {
        while e[x] != x {
            e[x] = e[e[x]];
            x = e[x];
        }
        x
    }
    for p in paare {
        let (a, b) = (w(&mut e, p.i), w(&mut e, p.j));
        e[a] = b;
    }
    (0..n).map(|x| w(&mut e, x)).collect()
}

/// Mitglieder der groessten zusammenhaengenden Gruppe (B&L Abschnitt 3: "the
/// connected component" — die groesste wird das Panorama).
pub fn groesste_gruppe(n: usize, paare: &[Paar]) -> Vec<usize> {
    let g = gruppen(n, paare);
    let mut groesse = vec![0usize; n];
    for &x in &g {
        groesse[x] += 1;
    }
    // Bei Gleichstand die Gruppe mit dem kleinsten Bildindex (reproduzierbar).
    let beste = (0..n).max_by_key(|&w| (groesse[w], std::cmp::Reverse(w))).unwrap_or(0);
    (0..n).filter(|&x| g[x] == beste).collect()
}

/// Mitte des staerksten Spannbaums (Kanten nach Zahl der stimmigen Punkte,
/// Mitte = kleinste groesste Entfernung zu den Blaettern) — wie
/// die Spannbaum-Suche der Referenzumsetzung von Brown & Lowe. Auf dieses
/// Bild wird die Lage vor der Begradigung bezogen.
pub fn spannbaum_mitte(n: usize, paare: &[Paar], mitglieder: &[usize]) -> usize {
    let mut eltern: Vec<usize> = (0..n).collect();
    fn w(e: &mut [usize], mut x: usize) -> usize {
        while e[x] != x {
            e[x] = e[e[x]];
            x = e[x];
        }
        x
    }
    let mut ordnung: Vec<usize> = (0..paare.len()).collect();
    ordnung.sort_by_key(|&k| std::cmp::Reverse(paare[k].inlier));
    let mut nachbarn = vec![Vec::new(); n];
    for k in ordnung {
        let (a, b) = (w(&mut eltern, paare[k].i), w(&mut eltern, paare[k].j));
        if a != b {
            eltern[a] = b;
            nachbarn[paare[k].i].push(paare[k].j);
            nachbarn[paare[k].j].push(paare[k].i);
        }
    }
    let mut beste = (usize::MAX, mitglieder[0]);
    for &s in mitglieder {
        let mut tiefe = vec![usize::MAX; n];
        tiefe[s] = 0;
        let mut schlange = std::collections::VecDeque::from([s]);
        let mut max = 0;
        while let Some(x) = schlange.pop_front() {
            for &y in &nachbarn[x] {
                if tiefe[y] == usize::MAX {
                    tiefe[y] = tiefe[x] + 1;
                    max = max.max(tiefe[y]);
                    schlange.push_back(y);
                }
            }
        }
        if max < beste.0 {
            beste = (max, s);
        }
    }
    beste.1
}

/// Optik eines Bildes: Brennweite und radiale Verzeichnung.
#[derive(Clone, Copy, Debug)]
pub struct Optik {
    pub f: f64,
    pub k1: f64,
}

/// Groesstes r^2, bis zu dem das Verzeichnungsmodell gilt. Darueber bleibt der
/// Faktor konstant: sonst faltet eine Tonnenverzeichnung weit ausserhalb des
/// Bildes zurueck, und Punkte jenseits des Rands landeten scheinbar im Bild.
const R2_MAX: f64 = 1.6;
pub const K1_GRENZEN: (f64, f64) = (-0.2, 0.3);

#[inline]
pub fn verzeichnen(x: f64, y: f64, k1: f64, d: f64) -> (f64, f64) {
    if k1 == 0.0 {
        return (x, y);
    }
    let r2 = ((x * x + y * y) / (d * d)).min(R2_MAX);
    let s = 1.0 + k1 * r2;
    (x * s, y * s)
}

#[inline]
pub fn entzeichnen(x: f64, y: f64, k1: f64, d: f64) -> (f64, f64) {
    if k1 == 0.0 {
        return (x, y);
    }
    // Fixpunkt-Iteration; im zulaessigen k1-Bereich kontrahierend.
    let (mut u, mut v) = (x, y);
    for _ in 0..12 {
        let r2 = ((u * u + v * v) / (d * d)).min(R2_MAX);
        let s = 1.0 + k1 * r2;
        u = x / s;
        v = y / s;
    }
    (u, v)
}

/// Rueckprojektion: Punkt aus Bild `von` (zentriert, wie gemessen — also
/// verzeichnet) nach Bild `nach` (B&L Gl. 15). `d_*`: halbe Bilddiagonalen.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn uebertragen(r_von: &Mat3, r_nach: &Mat3, o_von: Optik, o_nach: Optik, d_von: f64, d_nach: f64, x: f64, y: f64) -> Option<(f64, f64)> {
    let (x, y) = entzeichnen(x, y, o_von.k1, d_von);
    let w = mal_vek(r_von, &[x, y, o_von.f]);
    let c = [
        r_nach[0][0] * w[0] + r_nach[1][0] * w[1] + r_nach[2][0] * w[2],
        r_nach[0][1] * w[0] + r_nach[1][1] * w[1] + r_nach[2][1] * w[2],
        r_nach[0][2] * w[0] + r_nach[1][2] * w[1] + r_nach[2][2] * w[2],
    ];
    if c[2] <= 1e-9 * (c[0].abs() + c[1].abs() + c[2].abs()) {
        return None;
    }
    Some(verzeichnen(o_nach.f * c[0] / c[2], o_nach.f * c[1] / c[2], o_nach.k1, d_nach))
}

/// Das Kameramodell aller Bilder: Drehung je Bild, Brennweite je
/// Brennweitengruppe, k1 je Optikgruppe.
#[derive(Clone, Debug)]
pub struct Modell {
    pub r: Vec<Mat3>,
    pub f: Vec<f64>,
    pub k1: Vec<f64>,
    /// Brennweitengruppe je Bild.
    pub gruppe_f: Vec<usize>,
    /// Optikgruppe (k1) je Bild.
    pub gruppe_k: Vec<usize>,
    /// Darf k1 der Optikgruppe geschaetzt werden (nicht vorab entzerrt)?
    pub k1_frei: Vec<bool>,
    /// Darf die Brennweite der Gruppe geschaetzt werden? (Fest nach der
    /// Plausibilitaetsgrenze, siehe `brennweite_pruefen`.)
    pub f_frei: Vec<bool>,
    /// Halbe Bilddiagonale je Bild (Bezug der Verzeichnung), im Massstab der
    /// Punktkoordinaten.
    pub diag: Vec<f64>,
}

impl Modell {
    #[inline]
    pub fn optik(&self, k: usize) -> Optik {
        Optik { f: self.f[self.gruppe_f[k]], k1: self.k1[self.gruppe_k[k]] }
    }

    /// Dasselbe Modell fuer Punktkoordinaten im Massstab `s` (Brennweiten und
    /// Diagonalen mal s; Drehungen und k1 bleiben).
    pub fn skaliert(&self, s: f64) -> Modell {
        let mut m = self.clone();
        m.f.iter_mut().for_each(|f| *f *= s);
        m.diag.iter_mut().for_each(|d| *d *= s);
        m
    }
}

/// Restfehler eines Paars, beidseitig (B&L Gl. 14/16): 4 Werte je Punkt.
fn residuen(m: &Modell, p: &Paar, r_i: &Mat3, r_j: &Mat3, oi: Optik, oj: Optik, aus: &mut Vec<f64>) {
    aus.clear();
    let (di, dj) = (m.diag[p.i], m.diag[p.j]);
    for q in &p.punkte {
        match uebertragen(r_j, r_i, oj, oi, dj, di, q[2], q[3]) {
            Some((u, v)) => {
                aus.push(q[0] - u);
                aus.push(q[1] - v);
            }
            None => {
                aus.push(1e4);
                aus.push(1e4);
            }
        }
        match uebertragen(r_i, r_j, oi, oj, di, dj, q[0], q[1]) {
            Some((u, v)) => {
                aus.push(q[2] - u);
                aus.push(q[3] - v);
            }
            None => {
                aus.push(1e4);
                aus.push(1e4);
            }
        }
    }
}

/// Huber-Funktion nach B&L Gl. 17 fuer den Betrag e eines Restfehlers:
/// (Kosten, IRLS-Gewicht). σ = ∞: reine Quadratsumme.
#[inline]
fn huber(e: f64, sigma: f64) -> (f64, f64) {
    if e < sigma {
        (e * e, 1.0)
    } else {
        (2.0 * sigma * e - sigma * sigma, sigma / e)
    }
}

fn kosten(m: &Modell, paare: &[&Paar], sigma: f64) -> f64 {
    let mut puffer = Vec::new();
    let mut s = 0.0;
    for p in paare {
        residuen(m, p, &m.r[p.i], &m.r[p.j], m.optik(p.i), m.optik(p.j), &mut puffer);
        for c in puffer.chunks_exact(2) {
            s += huber(c[0].hypot(c[1]), sigma).0;
        }
    }
    s
}

/// Mittlerer Rueckprojektionsfehler (Pixel) ueber alle Punkte aller Paare.
pub fn mittlerer_fehler(m: &Modell, paare: &[Paar]) -> f64 {
    let mut puffer = Vec::new();
    let (mut s, mut n) = (0.0, 0usize);
    for p in paare {
        residuen(m, p, &m.r[p.i], &m.r[p.j], m.optik(p.i), m.optik(p.j), &mut puffer);
        for c in puffer.chunks_exact(2) {
            s += c[0].hypot(c[1]);
            n += 1;
        }
    }
    s / n.max(1) as f64
}

/// Levenberg-Marquardt nach B&L Gl. 18 ueber die Bilder `aktiv` (Drehungen
/// ausser `fest`), ihre Brennweitengruppen und freien k1. `sigma`: Huber-
/// Schwelle (Gl. 17). Rueckgabe: Endkosten.
#[allow(clippy::needless_range_loop)]
fn lm(m: &mut Modell, paare: &[&Paar], aktiv: &[usize], fest: usize, sigma: f64, max_iter: usize) -> f64 {
    let n = m.r.len();
    // Parameter: je aktivem Bild (ausser `fest`) 3 Drehwinkel, dann je
    // beteiligter Brennweitengruppe f, dann je freier Optikgruppe k1.
    let mut i_rot = vec![usize::MAX; n];
    let mut zahl = 0;
    for &k in aktiv {
        if k != fest {
            i_rot[k] = zahl;
            zahl += 3;
        }
    }
    let mut i_f = vec![usize::MAX; m.f.len()];
    let mut i_k = vec![usize::MAX; m.k1.len()];
    for &k in aktiv {
        let g = m.gruppe_f[k];
        if i_f[g] == usize::MAX && m.f_frei[g] {
            i_f[g] = zahl;
            zahl += 1;
        }
        let g = m.gruppe_k[k];
        if m.k1_frei[g] && i_k[g] == usize::MAX {
            i_k[g] = zahl;
            zahl += 1;
        }
    }
    let p_anz = zahl;
    let mut aktuell = kosten(m, paare, sigma);
    if p_anz == 0 || paare.is_empty() {
        return aktuell;
    }
    // Daempfung C_p^-1 (Gl. 19); σf = f̄/10 mit f̄ = Mittel der beteiligten
    // Brennweiten.
    let (fs, fz) = (0..m.f.len()).filter(|&g| i_f[g] != usize::MAX).fold((0.0f64, 0.0f64), |a, g| (a.0 + m.f[g], a.1 + 1.0));
    let f_mittel = fs / fz.max(1.0);
    let sigma_theta = std::f64::consts::PI / 16.0;
    let sigma_f = f_mittel / 10.0;
    const SIGMA_K: f64 = 0.1;
    let mut cp_inv = vec![0.0f64; p_anz];
    for &k in aktiv {
        if i_rot[k] != usize::MAX {
            for a in 0..3 {
                cp_inv[i_rot[k] + a] = 1.0 / (sigma_theta * sigma_theta);
            }
        }
    }
    for g in 0..m.f.len() {
        if i_f[g] != usize::MAX {
            cp_inv[i_f[g]] = 1.0 / (sigma_f * sigma_f);
        }
    }
    for g in 0..m.k1.len() {
        if i_k[g] != usize::MAX {
            cp_inv[i_k[g]] = 1.0 / (SIGMA_K * SIGMA_K);
        }
    }
    let anwenden = |m: &Modell, d: &[f64]| -> Modell {
        let mut neu = m.clone();
        for &k in aktiv {
            if i_rot[k] != usize::MAX {
                let q = i_rot[k];
                neu.r[k] = mul(&drehung(&[d[q], d[q + 1], d[q + 2]]), &m.r[k]);
            }
        }
        for g in 0..m.f.len() {
            if i_f[g] != usize::MAX {
                neu.f[g] = (m.f[g] + d[i_f[g]]).max(1e-3 * m.f[g]);
            }
        }
        for g in 0..m.k1.len() {
            if i_k[g] != usize::MAX {
                neu.k1[g] = (m.k1[g] + d[i_k[g]]).clamp(K1_GRENZEN.0, K1_GRENZEN.1);
            }
        }
        neu
    };
    let mut lambda = 1.0;
    let mut basis = Vec::new();
    let mut plus = Vec::new();
    let mut minus = Vec::new();
    // Lokale Parameter eines Paars: Drehung i (3), Drehung j (3), f_i, f_j, k_i, k_j.
    const L: usize = 10;
    for _ in 0..max_iter {
        let mut hm = vec![0.0f64; p_anz * p_anz];
        let mut g = vec![0.0f64; p_anz];
        for p in paare {
            let mut glob = [usize::MAX; L];
            if i_rot[p.i] != usize::MAX {
                for a in 0..3 {
                    glob[a] = i_rot[p.i] + a;
                }
            }
            if i_rot[p.j] != usize::MAX {
                for a in 0..3 {
                    glob[3 + a] = i_rot[p.j] + a;
                }
            }
            let (gfi, gfj) = (m.gruppe_f[p.i], m.gruppe_f[p.j]);
            let (gki, gkj) = (m.gruppe_k[p.i], m.gruppe_k[p.j]);
            glob[6] = i_f[gfi];
            glob[7] = if gfj == gfi { usize::MAX } else { i_f[gfj] };
            glob[8] = i_k[gki];
            glob[9] = if gkj == gki { usize::MAX } else { i_k[gkj] };
            let (oi, oj) = (m.optik(p.i), m.optik(p.j));
            residuen(m, p, &m.r[p.i], &m.r[p.j], oi, oj, &mut basis);
            let zeilen = basis.len();
            let mut jac = vec![0.0f64; zeilen * L];
            for q in 0..L {
                if glob[q] == usize::MAX {
                    continue;
                }
                let eps = match q {
                    6 => 1e-6 * oi.f,
                    7 => 1e-6 * oj.f,
                    _ => 1e-6,
                };
                let stoss = |s: f64| -> (Mat3, Mat3, Optik, Optik) {
                    let (mut ri, mut rj, mut a, mut b) = (m.r[p.i], m.r[p.j], oi, oj);
                    match q {
                        0..=2 => {
                            let mut v = [0.0; 3];
                            v[q] = s;
                            ri = mul(&drehung(&v), &ri);
                        }
                        3..=5 => {
                            let mut v = [0.0; 3];
                            v[q - 3] = s;
                            rj = mul(&drehung(&v), &rj);
                        }
                        6 => {
                            a.f += s;
                            if gfj == gfi {
                                b.f += s;
                            }
                        }
                        7 => b.f += s,
                        8 => {
                            a.k1 += s;
                            if gkj == gki {
                                b.k1 += s;
                            }
                        }
                        _ => b.k1 += s,
                    }
                    (ri, rj, a, b)
                };
                let (a1, b1, o1, p1) = stoss(eps);
                residuen(m, p, &a1, &b1, o1, p1, &mut plus);
                let (a2, b2, o2, p2) = stoss(-eps);
                residuen(m, p, &a2, &b2, o2, p2, &mut minus);
                for z in 0..zeilen {
                    jac[z * L + q] = (plus[z] - minus[z]) / (2.0 * eps);
                }
            }
            for z2 in 0..zeilen / 2 {
                let e = basis[2 * z2].hypot(basis[2 * z2 + 1]);
                let w = huber(e, sigma).1;
                for z in [2 * z2, 2 * z2 + 1] {
                    let zeile = &jac[z * L..z * L + L];
                    for a in 0..L {
                        if glob[a] == usize::MAX || zeile[a] == 0.0 {
                            continue;
                        }
                        g[glob[a]] += w * zeile[a] * basis[z];
                        for b in 0..L {
                            if glob[b] != usize::MAX {
                                hm[glob[a] * p_anz + glob[b]] += w * zeile[a] * zeile[b];
                            }
                        }
                    }
                }
            }
        }
        let mut verbessert = false;
        loop {
            let mut a = hm.clone();
            for d in 0..p_anz {
                a[d * p_anz + d] += lambda * cp_inv[d];
            }
            let rechts: Vec<f64> = g.iter().map(|v| -v).collect();
            if let Some(d) = cholesky_loesen(&a, &rechts, p_anz) {
                let neu = anwenden(m, &d);
                let c = kosten(&neu, paare, sigma);
                if c < aktuell {
                    let rel = (aktuell - c) / aktuell.max(1e-300);
                    *m = neu;
                    aktuell = c;
                    lambda = (lambda / 10.0).max(1e-12);
                    verbessert = rel > 1e-10;
                    break;
                }
            }
            lambda *= 10.0;
            if lambda > 1e16 {
                break;
            }
        }
        if !verbessert {
            break;
        }
    }
    aktuell
}

/// Buendelausgleich nach B&L Abschnitt 4: Bilder einzeln hinzufuegen (das
/// mit den meisten stimmigen Punktpaaren zu einem schon ausgerichteten Bild
/// zuerst, Start mit Drehung und Brennweite dieses Partners), nach jedem
/// Schritt LM mit σ = ∞, zuletzt LM mit σ = `sigma_ende` (2 px). Startbild:
/// das Bild mit den meisten stimmigen Punktpaaren insgesamt.
/// `m.r` wird ueberschrieben, `m.f`/`m.k1` sind Startwerte. Rueckgabe: das
/// Startbild (seine Drehung blieb fest).
pub fn buendelausgleich(m: &mut Modell, paare: &[Paar], mitglieder: &[usize], sigma_ende: f64) -> usize {
    let n = m.r.len();
    let mut summe = vec![0usize; n];
    for p in paare {
        summe[p.i] += p.inlier;
        summe[p.j] += p.inlier;
    }
    let start = *mitglieder.iter().max_by_key(|&&k| (summe[k], std::cmp::Reverse(k))).unwrap();
    let mut drin = vec![false; n];
    drin[start] = true;
    m.r[start] = EINHEIT;
    let mut aktiv = vec![start];
    while aktiv.len() < mitglieder.len() {
        // Bestes Paar zwischen drinnen und draussen.
        let Some(p) = paare
            .iter()
            .filter(|p| drin[p.i] != drin[p.j])
            .max_by_key(|p| (p.inlier, std::cmp::Reverse(p.i.min(p.j))))
        else {
            break;
        };
        let (neu, partner) = if drin[p.i] { (p.j, p.i) } else { (p.i, p.j) };
        m.r[neu] = m.r[partner];
        // Eigene Brennweitengruppe noch ohne Wert: die des Partners.
        let g_neu = m.gruppe_f[neu];
        if !aktiv.iter().any(|&k| m.gruppe_f[k] == g_neu) {
            m.f[g_neu] = m.f[m.gruppe_f[partner]];
        }
        drin[neu] = true;
        aktiv.push(neu);
        let teil: Vec<&Paar> = paare.iter().filter(|p| drin[p.i] && drin[p.j]).collect();
        let c = lm(m, &teil, &aktiv, start, f64::INFINITY, 100);
        super::diagnose(&format!("  Ausgleich: Bild {} dazu (Partner {}), Kosten {c:.1}, f {:?}", neu + 1, partner + 1, m.f));
    }
    let alle: Vec<&Paar> = paare.iter().filter(|p| drin[p.i] && drin[p.j]).collect();
    lm(m, &alle, &aktiv, start, sigma_ende, 200);
    start
}

/// Plausibilitaetsgrenze der Brennweite: der Prior von B&L (Gl. 19) hat
/// σf = f̄/10. Liegt eine geschaetzte Brennweite mehr als 3 σf (30 %) neben
/// der Brennweite aus den Metadaten, ist die Loesung entartet (gemessen:
/// Aufnahmen vom fahrenden Schiff — reine Verschiebung, die Brennweite laeuft
/// gegen unendlich, Reihe 2284–2288 f = 113 x lange Seite; oder sie faellt,
/// Reihe vom Schiff −35 %). Dann wird die Gruppe auf die Metadaten-Brennweite
/// festgelegt. `meta`: Metadaten-Brennweite je Gruppe (im Massstab des
/// Modells). Rueckgabe: ob eine Gruppe festgelegt wurde.
#[allow(clippy::needless_range_loop)]
pub fn brennweite_pruefen(m: &mut Modell, meta: &[Option<f64>]) -> bool {
    let mut fest = false;
    for g in 0..m.f.len() {
        if let Some(fm) = meta[g] {
            if m.f_frei[g] && (m.f[g] - fm).abs() > 3.0 * fm / 10.0 {
                m.f[g] = fm;
                m.f_frei[g] = false;
                fest = true;
            }
        }
    }
    fest
}

/// Nur die Endrunde (alle Bilder, σ = `sigma`), z. B. nach der Feinausrichtung.
pub fn ausgleich_ende(m: &mut Modell, paare: &[Paar], mitglieder: &[usize], fest: usize, sigma: f64) -> f64 {
    let alle: Vec<&Paar> = paare.iter().collect();
    lm(m, &alle, mitglieder, fest, sigma, 200)
}

/// Art der Begradigung (wie in der Referenzumsetzung).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Begradigung {
    /// Waagrechte Reihe/Raster: B&L Gl. 26.
    Waagrecht,
    /// Senkrechte Reihe: Bezugsachse ist die gemeinsame waagrechte Kameraachse.
    Senkrecht,
}

/// Lage auf das Mittelbild beziehen (wie der Buendelausgleich der
/// Referenzumsetzung nach dem Ausgleich).
pub fn auf_mitte_beziehen(r: &mut [Mat3], mitglieder: &[usize], mitte: usize) {
    let inv = transp(&r[mitte]);
    for &k in mitglieder {
        r[k] = mul(&inv, &r[k]);
    }
}

/// Erkennung waagrecht/senkrecht wie die automatische Erkennung der
/// Referenzumsetzung: Blickrichtungen (dritte Spalte von R) in die Ebene
/// z = 1 projiziert; streuen sie senkrecht staerker als waagrecht, ist es
/// eine senkrechte Reihe.
pub fn begradigung_erkennen(r: &[Mat3], mitglieder: &[usize]) -> Begradigung {
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for &k in mitglieder {
        let (x, y) = (r[k][0][2] / r[k][2][2], r[k][1][2] / r[k][2][2]);
        x0 = x0.min(x);
        x1 = x1.max(x);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    if y1 - y0 > x1 - x0 {
        Begradigung::Senkrecht
    } else {
        Begradigung::Waagrecht
    }
}

/// Begradigung nach B&L Abschnitt 5 / Gl. 26 und dem Vorgehen der
/// Referenzumsetzung. Waagrecht: der Aufwaertsvektor u ist der Nullvektor
/// von Σ X_i X_iᵀ (X_i = waagrechte Kameraachse, kleinster Eigenwert); die
/// Weltachsen werden x = u × Σ z_i, y = u (Vorzeichen so, dass Σ x·X_i > 0),
/// z = x × y. Senkrecht (entartet: alle X_i gleich): statt u die Hauptachse
/// von Σ X_i X_iᵀ (groesster Eigenwert), wie die Referenzumsetzung — die Reihe
/// laeuft dann auf der Leinwand QUER (siehe `Leinwand::quer`).
/// Unsere Welt: y zeigt nach unten (Bildzeilen), z in Blickrichtung. Ist
/// Σ z_i parallel zu u (kein Bezug), bleibt die Lage unveraendert — wie die
/// Referenzumsetzung.
pub fn begradigen(r: &mut [Mat3], mitglieder: &[usize], art: Begradigung) {
    let mut m = [0.0f64; 9];
    let mut z = [0.0f64; 3];
    for &k in mitglieder {
        let x = [r[k][0][0], r[k][1][0], r[k][2][0]];
        for a in 0..3 {
            for b in 0..3 {
                m[a * 3 + b] += x[a] * x[b];
            }
            z[a] += r[k][a][2];
        }
    }
    let (_, v) = eigen_sym(&m, 3); // aufsteigend
    let spalte = if art == Begradigung::Waagrecht { 0 } else { 2 };
    let mut rg1 = [v[spalte], v[3 + spalte], v[6 + spalte]];
    let mut rg0 = kreuz(&rg1, &z);
    let n0 = skalar(&rg0, &rg0).sqrt();
    if n0 <= f64::MIN_POSITIVE {
        return;
    }
    rg0 = [rg0[0] / n0, rg0[1] / n0, rg0[2] / n0];
    let rg2 = kreuz(&rg0, &rg1);
    let mut conf = 0.0;
    for &k in mitglieder {
        let x = [r[k][0][0], r[k][1][0], r[k][2][0]];
        conf += if art == Begradigung::Waagrecht { skalar(&rg0, &x) } else { -skalar(&rg1, &x) };
    }
    if conf < 0.0 {
        rg0 = [-rg0[0], -rg0[1], -rg0[2]];
        rg1 = [-rg1[0], -rg1[1], -rg1[2]];
    }
    let w = [rg0, rg1, rg2];
    for &k in mitglieder {
        r[k] = mul(&w, &r[k]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gl. 26: eine schief gehaltene Schwenkreihe (gerollt und geneigt) wird so
    /// gedreht, dass die waagrechten Kameraachsen waagrecht liegen.
    #[test]
    fn begradigung_legt_kameraachsen_waagrecht() {
        let schief = mul(&drehung(&[0.15, 0.0, 0.0]), &drehung(&[0.0, 0.0, 0.1]));
        let mut r: Vec<Mat3> = (-2..=2).map(|k| mul(&schief, &drehung(&[0.0, 0.3 * k as f64, 0.0]))).collect();
        let alle: Vec<usize> = (0..5).collect();
        assert_eq!(begradigung_erkennen(&r, &alle), Begradigung::Waagrecht);
        begradigen(&mut r, &alle, Begradigung::Waagrecht);
        for k in &r {
            assert!(k[1][0].abs() < 1e-9, "{k:?}");
            assert!(k[0][0] > 0.0);
        }
    }
}
