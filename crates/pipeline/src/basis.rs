//! Kantenerhaltende Basisebene — das, was Lichter und Tiefen „lokal" macht.
//!
//! Die Referenz-Entwicklung hebt mit „Tiefen +50" nicht einfach alle dunklen
//! Pixel an (das waere eine globale Kurve und drueckt den Kontrast platt),
//! sondern dunkle BEREICHE — die Zeichnung darin bleibt erhalten. Dafuer braucht man eine
//! Basisebene: die Helligkeit der Umgebung jedes Pixels, die an Kanten nicht
//! verschmiert (sonst Lichthoefe um Personen vor hellem Himmel).
//!
//! Verfahren: gefuehrter Filter (He, Sun, Tang) auf der log2-Helligkeit,
//! gerechnet auf einer festen Verkleinerung des GANZEN Bildes (`BASIS_MAX`).
//! Gespeichert werden nicht die geglaetteten Werte, sondern die linearen
//! Koeffizienten (a, b) — die GPU rechnet daraus mit der Helligkeit des
//! Pixels in ihrer eigenen Aufloesung `basis = a · L + b` (schneller gefuehrter
//! Filter). Dadurch ist die Basis in Vorschau, 1:1-Ausschnitt und Export
//! dieselbe und folgt Kanten pixelgenau — der Fehler der alten Klarheit
//! (feste Pixelradien, Vorschau ≠ Export) kann hier gar nicht entstehen.
//!
//! Zwei Massstaebe: ein grosser fuer Lichter/Tiefen, ein mittlerer fuer
//! Klarheit.

/// Lange Kante der Basis-Verkleinerung. Fest — unabhaengig davon, ob gerade
/// Vorschau, Zoom oder Export rechnet.
pub const BASIS_MAX: u32 = 1024;

/// Filtergroessen relativ zur langen Bildkante, Glaettung in log2-Einheiten².
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasisParameter {
    pub radius_ton: f32,
    pub eps_ton: f32,
    pub radius_klar: f32,
    pub eps_klar: f32,
}

impl Default for BasisParameter {
    fn default() -> Self {
        Self {
            radius_ton: 0.025,
            eps_ton: 0.06,
            radius_klar: 0.007,
            eps_klar: 0.03,
        }
    }
}

/// Die Verkleinerung des ganzen Bildes, aus der die Basis berechnet wird:
/// lineare Kamerawerte (wie LibRaw sie liefert), RGB je Pixel.
#[derive(Debug, Clone)]
pub struct BasisQuelle {
    pub breite: u32,
    pub hoehe: u32,
    pub rgb: Vec<[f32; 3]>,
}

impl BasisQuelle {
    /// Flaechentreue Verkleinerung einer f16-RGBA-Textur (ganzes Bild) auf
    /// genau `BASIS_MAX` an der langen Kante. Vorschau (2400 px) und Export
    /// (volle Groesse) landen dadurch auf demselben Raster.
    pub fn aus_f16(daten: &[u16], w: u32, h: u32) -> Self {
        let lang = w.max(h).max(1);
        let s = (BASIS_MAX as f32 / lang as f32).min(1.0);
        let bw = ((w as f32 * s).round() as u32).max(1);
        let bh = ((h as f32 * s).round() as u32).max(1);
        let mut rgb = vec![[0f32; 3]; (bw * bh) as usize];
        let fx = w as f32 / bw as f32;
        let fy = h as f32 / bh as f32;
        let wert = |x: u32, y: u32, c: usize| -> f32 {
            half::f16::from_bits(daten[((y * w + x) * 4) as usize + c]).to_f32()
        };
        for by in 0..bh {
            let y0 = by as f32 * fy;
            let y1 = (by + 1) as f32 * fy;
            for bx in 0..bw {
                let x0 = bx as f32 * fx;
                let x1 = (bx + 1) as f32 * fx;
                let mut acc = [0f32; 3];
                let mut gew = 0f32;
                let mut y = y0.floor() as u32;
                while (y as f32) < y1 && y < h {
                    let wy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
                    let mut x = x0.floor() as u32;
                    while (x as f32) < x1 && x < w {
                        let wx = ((x + 1) as f32).min(x1) - (x as f32).max(x0);
                        let g = wx * wy;
                        for c in 0..3 {
                            acc[c] += wert(x, y, c) * g;
                        }
                        gew += g;
                        x += 1;
                    }
                    y += 1;
                }
                let i = (by * bw + bx) as usize;
                for c in 0..3 {
                    rgb[i][c] = acc[c] / gew.max(1e-9);
                }
            }
        }
        Self {
            breite: bw,
            hoehe: bh,
            rgb,
        }
    }
}

/// Fertige Koeffizienten: je Basispixel (a_ton, b_ton, a_klar, b_klar).
#[derive(Debug, Clone)]
pub struct Basis {
    pub breite: u32,
    pub hoehe: u32,
    pub koeff: Vec<[f32; 4]>,
    /// log2-Helligkeit (ohne Belichtung): 2., 50., 98. und 99,5. Perzentil —
    /// damit Lichter und Tiefen sich dem Kontrastumfang des Bildes anpassen
    /// („bildabhaengig" wie in der Referenz-Entwicklung).
    pub perzentile: [f32; 4],
}

/// Kleinste Helligkeit, die in den Logarithmus geht (~16 Blenden unter Weiss).
pub const LOG_BODEN: f32 = 1.0 / 65536.0;

impl Basis {
    /// Basis aus der Quelle mit der aktuellen Farbumrechnung (die Helligkeit
    /// haengt vom Weissabgleich ab). `y_zeile` = Helligkeitszeile der Matrix
    /// Kamera → ROMM-RGB (also Y der Kamerawerte).
    pub fn berechnen(q: &BasisQuelle, y_zeile: [f32; 3], p: &BasisParameter) -> Self {
        let (w, h) = (q.breite as usize, q.hoehe as usize);
        let l: Vec<f32> = q
            .rgb
            .iter()
            .map(|c| (c[0] * y_zeile[0] + c[1] * y_zeile[1] + c[2] * y_zeile[2]).max(LOG_BODEN).log2())
            .collect();
        let lang = w.max(h) as f32;
        let r_ton = ((p.radius_ton * lang).round() as usize).max(1);
        let r_klar = ((p.radius_klar * lang).round() as usize).max(1);
        let (a1, b1) = gefuehrt(&l, w, h, r_ton, p.eps_ton);
        let (a2, b2) = gefuehrt(&l, w, h, r_klar, p.eps_klar);
        let koeff = (0..w * h).map(|i| [a1[i], b1[i], a2[i], b2[i]]).collect();
        // Perzentile auf der GEGLAETTETEN Basis (Umgebungshelligkeit) — einzelne
        // Spitzlichter oder Rauschpixel sollen den Umfang nicht bestimmen.
        let mut basis: Vec<f32> = (0..w * h).map(|i| a1[i] * l[i] + b1[i]).collect();
        let mut perz = |q: f32| -> f32 {
            let k = ((basis.len() as f32 * q) as usize).min(basis.len() - 1);
            let (_, v, _) = basis.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            *v
        };
        let perzentile = [perz(0.02), perz(0.50), perz(0.98), perz(0.995)];
        Self {
            breite: q.breite,
            hoehe: q.hoehe,
            koeff,
            perzentile,
        }
    }

    /// Neutrale Basis (a = 1, b = 0): Basis = Pixel, keine Lokalwirkung.
    pub fn neutral() -> Self {
        Self {
            breite: 1,
            hoehe: 1,
            koeff: vec![[1.0, 0.0, 1.0, 0.0]],
            perzentile: [-8.0, -3.0, -0.5, 0.0],
        }
    }

    /// Basiswert an einer Stelle (Anteil 0..1) fuer die Helligkeit `l`
    /// (log2) — derselbe Weg wie im Schader, fuer Tests und Miniaturen.
    pub fn ton_an(&self, u: f32, v: f32, l: f32) -> f32 {
        let k = self.abtasten(u, v);
        k[0] * l + k[1]
    }

    pub fn abtasten(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u * self.breite as f32 - 0.5).clamp(0.0, (self.breite - 1) as f32);
        let y = (v * self.hoehe as f32 - 0.5).clamp(0.0, (self.hoehe - 1) as f32);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.breite - 1), (y0 + 1).min(self.hoehe - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let g = |xx: u32, yy: u32| self.koeff[(yy * self.breite + xx) as usize];
        let (a, b, c, d) = (g(x0, y0), g(x1, y0), g(x0, y1), g(x1, y1));
        let mut o = [0f32; 4];
        for i in 0..4 {
            let oben = a[i] + (b[i] - a[i]) * fx;
            let unten = c[i] + (d[i] - c[i]) * fx;
            o[i] = oben + (unten - oben) * fy;
        }
        o
    }
}

/// Selbstgefuehrter Filter: liefert die geglaetteten Koeffizienten (ā, b̄).
fn gefuehrt(l: &[f32], w: usize, h: usize, r: usize, eps: f32) -> (Vec<f32>, Vec<f32>) {
    let mean = kasten(l, w, h, r);
    let sq: Vec<f32> = l.iter().map(|v| v * v).collect();
    let mean_sq = kasten(&sq, w, h, r);
    let mut a = vec![0f32; w * h];
    let mut b = vec![0f32; w * h];
    for i in 0..w * h {
        let var = (mean_sq[i] - mean[i] * mean[i]).max(0.0);
        a[i] = var / (var + eps);
        b[i] = mean[i] - a[i] * mean[i];
    }
    (kasten(&a, w, h, r), kasten(&b, w, h, r))
}

/// Kastenmittel mit Radius r ueber ein Summenbild (Rand: nur echte Pixel).
fn kasten(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut s = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut zeile = 0f64;
        for x in 0..w {
            zeile += v[y * w + x] as f64;
            s[(y + 1) * (w + 1) + x + 1] = s[y * (w + 1) + x + 1] + zeile;
        }
    }
    let mut o = vec![0f32; w * h];
    for y in 0..h {
        let y0 = y.saturating_sub(r);
        let y1 = (y + r + 1).min(h);
        for x in 0..w {
            let x0 = x.saturating_sub(r);
            let x1 = (x + r + 1).min(w);
            let summe = s[y1 * (w + 1) + x1] - s[y0 * (w + 1) + x1] - s[y1 * (w + 1) + x0]
                + s[y0 * (w + 1) + x0];
            o[y * w + x] = (summe / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quelle(w: u32, h: u32, f: impl Fn(u32, u32) -> f32) -> BasisQuelle {
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                rgb.push([v, v, v]);
            }
        }
        BasisQuelle { breite: w, hoehe: h, rgb }
    }

    /// Eine harte Kante (Himmel neben Person) bleibt in der Basis eine Kante —
    /// sonst entstehen Lichthoefe, sobald Lichter oder Tiefen wirken.
    #[test]
    fn kante_bleibt_kante() {
        let q = quelle(200, 100, |x, _| if x < 100 { 0.02 } else { 0.6 });
        let b = Basis::berechnen(&q, [1.0, 0.0, 0.0], &BasisParameter::default());
        let l_dunkel = 0.02f32.log2();
        let l_hell = 0.6f32.log2();
        // Zwei Pixel links und rechts der Kante: die Basis folgt der Kante.
        let links = b.ton_an(97.5 / 200.0, 0.5, l_dunkel);
        let rechts = b.ton_an(102.5 / 200.0, 0.5, l_hell);
        assert!((links - l_dunkel).abs() < 0.35, "{links} vs {l_dunkel}");
        assert!((rechts - l_hell).abs() < 0.35, "{rechts} vs {l_hell}");
    }

    /// Feines Rauschen/Struktur verschwindet in der Basis — die Zeichnung
    /// bleibt als Detail uebrig und wird von Lichtern/Tiefen nicht plattgedrueckt.
    #[test]
    fn feine_struktur_glaettet_sich() {
        let q = quelle(200, 100, |x, y| if (x + y) % 2 == 0 { 0.10 } else { 0.12 });
        let b = Basis::berechnen(&q, [1.0, 0.0, 0.0], &BasisParameter::default());
        let mitte = ((0.10f32.log2() + 0.12f32.log2()) / 2.0, 0.12f32.log2());
        let wert = b.ton_an(0.5, 0.5, mitte.1);
        assert!((wert - mitte.0).abs() < 0.05, "{wert} sollte ~{} sein", mitte.0);
    }
}
