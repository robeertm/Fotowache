//! Die Automatik — Fotowaches Gegenstueck zum „Auto" der Referenz-Entwicklung.
//!
//! Deren Automatik ist ein gelerntes Modell; veroeffentlicht ist nichts.
//! Hier wird sie aus echten Bearbeitungen der Referenz-Entwicklung gelernt: zu
//! jedem RAW gibt es in der XMP die Werte, bei denen der Fotograf dort gelandet
//! ist (fast immer die Auto-Werte). Aus einfachen Bildmerkmalen — wie hell ist
//! das Bild in Mitten, Lichtern und Tiefen, wie viel ist ausgefressen, wie bunt —
//! sagt eine lineare Regression je Regler den Wert der Referenz-Entwicklung vorher
//! (`--example automerkmale` + `scripts/auto_lernen.py`).
//!
//! Die Merkmale werden in genau dem Raum gemessen, in dem auch entwickelt wird
//! (Kameraprofil, Weissabgleich der Aufnahme, Grundbelichtung der
//! Standardentwicklung) — so bedeutet „Belichtung +0,3" hier dasselbe wie in
//! der Referenz-Entwicklung.

use crate::dcp;

/// Anzahl der Merkmale (Reihenfolge = `merkmale()`).
pub const ANZAHL: usize = 21;

/// Bildmerkmale eines RAW (ohne Nutzerregler).
pub fn merkmale(raw: &rawcore::RawImage) -> Vec<f32> {
    let profil = dcp::fuer_meta(&raw.meta);
    let fs = profil.farbstand(&raw.meta.wb_mul, &raw.meta.wb_aufnahme, None);
    let m = fs.kamera_zu_pro;
    // Ohne die kalibrierte Grundbelichtung: sonst verschoebe jede
    // Nachkalibrierung alle Merkmale — und die gelernte Automatik mit.
    let gain = (crate::zusatz_ev_fuer(&raw.meta, &profil) - crate::kalibrierung().grund_ev).exp2();
    let (w, h) = (raw.width as usize, raw.height as usize);
    let schritt = ((w * h / 150_000).max(1) as f32).sqrt().ceil() as usize;
    let mut l: Vec<f32> = Vec::with_capacity(160_000);
    let mut mitte = (0f64, 0f64);
    let mut rand = (0f64, 0f64);
    let mut satt = Vec::with_capacity(160_000);
    let mut ausgefressen = 0usize;
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = (y * w + x) * 3;
            let c = [
                raw.pixels[i] as f32 / 65535.0,
                raw.pixels[i + 1] as f32 / 65535.0,
                raw.pixels[i + 2] as f32 / 65535.0,
            ];
            let mut p = [0f32; 3];
            for r in 0..3 {
                p[r] = ((0..3).map(|k| m[r][k] * c[k]).sum::<f32>() * gain).max(0.0);
            }
            let lum = 0.2880402 * p[0] + 0.7118741 * p[1] + 0.0000857 * p[2];
            let ll = lum.max(1.0 / 65536.0).log2();
            l.push(ll);
            let mx = p[0].max(p[1]).max(p[2]);
            let mn = p[0].min(p[1]).min(p[2]);
            if mx > 1e-4 {
                satt.push((mx - mn) / mx);
            }
            if mx >= 0.98 {
                ausgefressen += 1;
            }
            let (fx, fy) = (x as f32 / w as f32 - 0.5, y as f32 / h as f32 - 0.5);
            if fx.abs() < 0.25 && fy.abs() < 0.25 {
                mitte.0 += ll as f64;
                mitte.1 += 1.0;
            } else {
                rand.0 += ll as f64;
                rand.1 += 1.0;
            }
            x += schritt;
        }
        y += schritt;
    }
    if l.is_empty() {
        return vec![0.0; ANZAHL];
    }
    let n = l.len() as f32;
    let mittel = l.iter().sum::<f32>() / n;
    let streu = (l.iter().map(|v| (v - mittel).powi(2)).sum::<f32>() / n).sqrt();
    let mut sortiert = l.clone();
    sortiert.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q = |p: f32| sortiert[((sortiert.len() as f32 * p) as usize).min(sortiert.len() - 1)];
    satt.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sq = |p: f32| if satt.is_empty() { 0.0 } else { satt[((satt.len() as f32 * p) as usize).min(satt.len() - 1)] };
    let dunkel = l.iter().filter(|v| **v < -6.0).count() as f32 / n;
    let hell = l.iter().filter(|v| **v > -0.5).count() as f32 / n;
    vec![
        q(0.01),
        q(0.05),
        q(0.10),
        q(0.25),
        q(0.50),
        q(0.75),
        q(0.90),
        q(0.95),
        q(0.98),
        q(0.99),
        q(0.995),
        q(0.999),
        mittel,
        streu,
        ausgefressen as f32 / n,
        (mitte.0 / mitte.1.max(1.0)) as f32,
        (rand.0 / rand.1.max(1.0)) as f32,
        sq(0.5),
        sq(0.9),
        dunkel,
        hell,
    ]
}

/// Die vorgeschlagenen Werte (Einheiten der XMP-Einstellungen; Belichtung in Blenden).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vorschlag {
    pub belichtung_ev: f32,
    pub kontrast: f32,
    pub lichter: f32,
    pub tiefen: f32,
    pub weiss: f32,
    pub schwarz: f32,
    pub dynamik: f32,
    pub saettigung: f32,
}

/// Gelernte Regression: je Regler Achsenabschnitt + Gewichte auf die
/// standardisierten Merkmale (Mittelwert/Streuung je Merkmal).
pub struct Modell {
    pub mittel: [f32; ANZAHL],
    pub streu: [f32; ANZAHL],
    /// Je Regler: [Achsenabschnitt, Gewicht 0..ANZAHL]
    pub gewichte: [[f32; ANZAHL + 1]; 8],
    /// Grenzen je Regler (Bereiche der XMP-Einstellungen, aus den Lerndaten).
    pub grenzen: [(f32, f32); 8],
}

include!("auto_modell.rs");

pub fn vorschlag(raw: &rawcore::RawImage) -> Vorschlag {
    vorschlag_aus(&merkmale(raw))
}

pub fn vorschlag_aus(mk: &[f32]) -> Vorschlag {
    let modell = &MODELL;
    let mut v = [0f32; 8];
    for (r, g) in modell.gewichte.iter().enumerate() {
        let mut s = g[0];
        for i in 0..ANZAHL.min(mk.len()) {
            let z = (mk[i] - modell.mittel[i]) / modell.streu[i].max(1e-6);
            s += g[i + 1] * z;
        }
        let (lo, hi) = modell.grenzen[r];
        v[r] = s.clamp(lo, hi);
    }
    Vorschlag {
        belichtung_ev: v[0],
        kontrast: v[1].round(),
        lichter: v[2].round(),
        tiefen: v[3].round(),
        weiss: v[4].round(),
        schwarz: v[5].round(),
        dynamik: v[6].round(),
        saettigung: v[7].round(),
    }
}
