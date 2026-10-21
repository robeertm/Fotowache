//! Hinzufuegen — Dinge ins Foto einsetzen, die nicht da waren (Wolken am
//! klaren Himmel …). Erzeugt wird das Stueck vom lokalen Bildgenerator der
//! Anwendung; hier wird nur ausgeschnitten und wieder eingesetzt.
//!
//! ## Warum auf den Rohdaten
//! Wie die Retusche sitzt das eingesetzte Stueck VOR allen Reglern: es wird mit
//! demselben Weissabgleich, derselben Belichtung und denselben Farben
//! entwickelt wie das echte Foto und veraendert sich mit, wenn man spaeter
//! an den Reglern dreht. Dafuer bekommt der Generator den Ausschnitt so, wie
//! er in den Rohdaten liegt (auf die hellen Werte normiert, mit Gamma
//! gestaucht), und das Ergebnis wird exakt zurueckgerechnet.
//!
//! ## Warum mit Maske
//! Der Generator rechnet den ganzen Ausschnitt neu und verschiebt dabei
//! Kleinigkeiten (gemessen: Gebaeudekanten am Rand). Uebernommen wird deshalb
//! nur der markierte Bereich, mit weichem Rand.

use std::sync::Arc;

/// Ein erzeugtes Stueck, fertig zum Einsetzen.
#[derive(Debug, Clone)]
pub struct Stueck {
    /// Bereich im ganzen Bild als Anteile: x0, y0, x1, y1.
    pub kasten: [f32; 4],
    /// Normierung beim Ausschneiden (linearer Wert, der 1,0 entsprach).
    pub weiss: f32,
    /// Markierte Stellen [x, y, radius] als Anteile (Radius an der Bildbreite).
    pub flecken: Vec<[f32; 3]>,
    /// Das erzeugte Bild (RGB, 8 Bit, Gamma 2,2) in der Groesse des Auftrags.
    pub bild: Arc<image::RgbImage>,
}

const GAMMA: f32 = 2.2;

/// Schneidet den Bereich als Vorlage fuer den Generator aus: hoechstens
/// `max_seite` Pixel lang, Seiten durch 16 teilbar. Gibt Bild und Normierung.
pub fn ausschneiden(data: &[u16], w: u32, h: u32, kasten: [f32; 4], max_seite: u32) -> (image::RgbImage, f32) {
    let (x0, y0, x1, y1) = pixelkasten(w, h, kasten);
    let (bw, bh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let s = (max_seite as f32 / bw.max(bh)).min(1.0);
    let ow = (((bw * s) as u32) / 16 * 16).max(16);
    let oh = (((bh * s) as u32) / 16 * 16).max(16);
    let px = |x: f32, y: f32, c: usize| -> f32 {
        let xi = (x as usize).min(w as usize - 1);
        let yi = (y as usize).min(h as usize - 1);
        half::f16::from_bits(data[(yi * w as usize + xi) * 4 + c]).to_f32().max(0.0)
    };
    // Normierung auf die hellen Werte des Ausschnitts.
    let mut hell: Vec<f32> = Vec::with_capacity((ow * oh) as usize);
    for y in 0..oh {
        for x in 0..ow {
            let sx = x0 as f32 + (x as f32 + 0.5) * bw / ow as f32;
            let sy = y0 as f32 + (y as f32 + 0.5) * bh / oh as f32;
            hell.push(px(sx, sy, 0).max(px(sx, sy, 1)).max(px(sx, sy, 2)));
        }
    }
    let mut sortiert = hell.clone();
    sortiert.sort_by(|a, b| a.total_cmp(b));
    let weiss = sortiert[((sortiert.len() as f32) * 0.995) as usize].max(1e-4);
    let mut bild = image::RgbImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let sx = x0 as f32 + (x as f32 + 0.5) * bw / ow as f32;
            let sy = y0 as f32 + (y as f32 + 0.5) * bh / oh as f32;
            let p = [0, 1, 2].map(|c| ((px(sx, sy, c) / weiss).clamp(0.0, 1.0).powf(1.0 / GAMMA) * 255.0 + 0.5) as u8);
            bild.put_pixel(x, y, image::Rgb(p));
        }
    }
    (bild, weiss)
}

fn pixelkasten(w: u32, h: u32, k: [f32; 4]) -> (u32, u32, u32, u32) {
    let x0 = ((k[0].clamp(0.0, 1.0) * w as f32) as u32).min(w - 1);
    let y0 = ((k[1].clamp(0.0, 1.0) * h as f32) as u32).min(h - 1);
    let x1 = ((k[2].clamp(0.0, 1.0) * w as f32).ceil() as u32).clamp(x0 + 1, w);
    let y1 = ((k[3].clamp(0.0, 1.0) * h as f32).ceil() as u32).clamp(y0 + 1, h);
    (x0, y0, x1, y1)
}

/// Gleicht die Farben des erzeugten Bildes an die Vorlage an.
///
/// Der Generator bekommt die flachen Rohdaten und liefert gern einen
/// „normalen" Fotohimmel zurueck — nach dem Entwickeln stand der eingesetzte
/// Bereich dann tiefblau neben dem echten Himmel (gemessen). Deshalb: je Kanal
/// der Median des Verhaeltnisses erzeugt/Vorlage (linear) im markierten
/// Bereich — der Median, damit das neu Hinzugekommene (die Wolken) die Messung
/// nicht verzieht — und das ganze Ergebnis damit zurueckgerechnet.
pub fn farben_angleichen(erzeugt: &mut image::RgbImage, vorlage: &image::RgbImage, maske: &dyn Fn(u32, u32) -> bool) {
    if erzeugt.dimensions() != vorlage.dimensions() {
        return;
    }
    let lin = |v: u8| (v as f32 / 255.0).powf(GAMMA);
    let mut verhaeltnis: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (x, y, p) in erzeugt.enumerate_pixels() {
        if !maske(x, y) {
            continue;
        }
        let o = vorlage.get_pixel(x, y);
        for c in 0..3 {
            let (e, v) = (lin(p[c]), lin(o[c]));
            if v > 0.002 && e > 0.002 {
                verhaeltnis[c].push(e / v);
            }
        }
    }
    let faktor: Vec<f32> = verhaeltnis
        .iter_mut()
        .map(|v| {
            if v.len() < 100 {
                return 1.0;
            }
            v.sort_by(|a, b| a.total_cmp(b));
            v[v.len() / 2].clamp(0.25, 4.0)
        })
        .collect();
    for p in erzeugt.pixels_mut() {
        for c in 0..3 {
            let v = (lin(p[c]) / faktor[c]).clamp(0.0, 1.0).powf(1.0 / GAMMA);
            p[c] = (v * 255.0 + 0.5) as u8;
        }
    }
}

/// Setzt ein Stueck in die Rohdaten (RGBA f16) ein — nur innerhalb der
/// markierten Stellen, mit weichem Rand.
pub fn einsetzen(data: &mut [u16], w: u32, h: u32, stueck: &Stueck) {
    // Abbildung ueber den UNGEKLEMMTEN Kasten (im Zoom-Ausschnitt ragt er
    // ueber den Rand), bearbeitet wird nur, was im Bild liegt.
    let kx0 = stueck.kasten[0] * w as f32;
    let ky0 = stueck.kasten[1] * h as f32;
    let bw = ((stueck.kasten[2] - stueck.kasten[0]) * w as f32).max(1.0);
    let bh = ((stueck.kasten[3] - stueck.kasten[1]) * h as f32).max(1.0);
    let x0 = kx0.max(0.0) as u32;
    let y0 = ky0.max(0.0) as u32;
    let x1 = ((kx0 + bw).ceil().max(0.0) as u32).min(w);
    let y1 = ((ky0 + bh).ceil().max(0.0) as u32).min(h);
    let (gw, gh) = (stueck.bild.width() as f32, stueck.bild.height() as f32);
    let flecken: Vec<(f32, f32, f32)> = stueck
        .flecken
        .iter()
        .map(|[x, y, r]| (x * w as f32, y * h as f32, (r * w as f32).max(2.0)))
        .collect();
    let rand = (bw.max(bh) * 0.02).max(3.0);
    let lese = |x: f32, y: f32, c: usize| -> f32 {
        let x = x.clamp(0.0, gw - 1.0);
        let y = y.clamp(0.0, gh - 1.0);
        let (xa, ya) = (x.floor() as u32, y.floor() as u32);
        let (xb, yb) = ((xa + 1).min(gw as u32 - 1), (ya + 1).min(gh as u32 - 1));
        let (fx, fy) = (x - xa as f32, y - ya as f32);
        let v = |xx: u32, yy: u32| stueck.bild.get_pixel(xx, yy)[c] as f32;
        let o = v(xa, ya) * (1.0 - fx) + v(xb, ya) * fx;
        let u = v(xa, yb) * (1.0 - fx) + v(xb, yb) * fx;
        ((o * (1.0 - fy) + u * fy) / 255.0).powf(GAMMA) * stueck.weiss
    };
    for yy in y0..y1 {
        for xx in x0..x1 {
            let (px, py) = (xx as f32 + 0.5, yy as f32 + 0.5);
            let mut a = 0.0f32;
            for &(fx, fy, r) in &flecken {
                let d = ((px - fx).powi(2) + (py - fy).powi(2)).sqrt();
                let t = ((r + rand - d) / rand).clamp(0.0, 1.0);
                a = a.max(t * t * (3.0 - 2.0 * t));
                if a >= 1.0 {
                    break;
                }
            }
            if a <= 0.0 {
                continue;
            }
            let gx = (px - kx0) / bw * gw - 0.5;
            let gy = (py - ky0) / bh * gh - 0.5;
            let i = (yy as usize * w as usize + xx as usize) * 4;
            for c in 0..3 {
                let alt = half::f16::from_bits(data[i + c]).to_f32();
                data[i + c] = half::f16::from_f32(alt * (1.0 - a) + lese(gx, gy, c) * a).to_bits();
            }
        }
    }
}

impl Stueck {
    /// Dasselbe Stueck im Bezug eines Ausschnitts (x, y, Breite, Hoehe in Pixeln
    /// des ganzen Bildes) — fuer den 1:1-Zoom.
    pub fn im_ausschnitt(&self, voll_w: u32, voll_h: u32, x: u32, y: u32, w: u32, h: u32) -> Stueck {
        let fx = |v: f32| (v * voll_w as f32 - x as f32) / w as f32;
        let fy = |v: f32| (v * voll_h as f32 - y as f32) / h as f32;
        Stueck {
            kasten: [fx(self.kasten[0]), fy(self.kasten[1]), fx(self.kasten[2]), fy(self.kasten[3])],
            weiss: self.weiss,
            flecken: self.flecken.iter().map(|[a, b, r]| [fx(*a), fy(*b), r * voll_w as f32 / w as f32]).collect(),
            bild: self.bild.clone(),
        }
    }
}

/// Der Kasten um alle markierten Stellen, mit Rand fuer Umgebung — als
/// Anteile am Bild.
pub fn kasten_um(flecken: &[[f32; 3]], seitenverhaeltnis: f32) -> [f32; 4] {
    let mut k = [1.0f32, 1.0, 0.0, 0.0];
    for [x, y, r] in flecken {
        // Radius ist an der Bildbreite gemessen; in y-Anteilen umrechnen.
        let ry = r * seitenverhaeltnis;
        k[0] = k[0].min(x - r);
        k[1] = k[1].min(y - ry);
        k[2] = k[2].max(x + r);
        k[3] = k[3].max(y + ry);
    }
    // Ein Drittel Umgebung auf jeder Seite.
    let (dx, dy) = ((k[2] - k[0]) / 3.0, (k[3] - k[1]) / 3.0);
    [(k[0] - dx).max(0.0), (k[1] - dy).max(0.0), (k[2] + dx).min(1.0), (k[3] + dy).min(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grau(w: u32, h: u32, v: f32) -> Vec<u16> {
        let b = half::f16::from_f32(v).to_bits();
        let eins = half::f16::from_f32(1.0).to_bits();
        (0..w * h).flat_map(|_| [b, b, b, eins]).collect()
    }

    #[test]
    fn ausschneiden_und_einsetzen_ist_verlustarm() {
        let (w, h) = (200, 100);
        let mut data = grau(w, h, 0.3);
        let kasten = [0.1, 0.1, 0.9, 0.9];
        let (bild, weiss) = ausschneiden(&data, w, h, kasten, 1024);
        assert_eq!(bild.width() % 16, 0);
        let stueck = Stueck { kasten, weiss, flecken: vec![[0.5, 0.5, 0.2]], bild: Arc::new(bild) };
        einsetzen(&mut data, w, h, &stueck);
        let v = half::f16::from_bits(data[((50 * w + 100) * 4) as usize]).to_f32();
        assert!((v - 0.3).abs() < 0.01, "{v}");
    }

    #[test]
    fn nur_die_markierte_stelle_aendert_sich() {
        let (w, h) = (200, 100);
        let mut data = grau(w, h, 0.3);
        let weiss_bild = image::RgbImage::from_pixel(160, 80, image::Rgb([255, 255, 255]));
        let stueck = Stueck { kasten: [0.0, 0.0, 1.0, 1.0], weiss: 1.0, flecken: vec![[0.25, 0.5, 0.05]], bild: Arc::new(weiss_bild) };
        einsetzen(&mut data, w, h, &stueck);
        let bei = |x: u32, y: u32| half::f16::from_bits(data[((y * w + x) * 4) as usize]).to_f32();
        assert!(bei(50, 50) > 0.9, "Mitte der Stelle wird hell");
        assert!((bei(150, 50) - 0.3).abs() < 1e-3, "weit weg bleibt");
    }

    #[test]
    fn farbstich_des_generators_wird_herausgerechnet() {
        let vorlage = image::RgbImage::from_pixel(64, 64, image::Rgb([120, 130, 140]));
        // Erzeugt: Himmel blauer, dazu eine helle „Wolke" in einer Ecke.
        let mut erzeugt = image::RgbImage::from_pixel(64, 64, image::Rgb([100, 125, 190]));
        for y in 0..16 {
            for x in 0..16 {
                erzeugt.put_pixel(x, y, image::Rgb([250, 250, 250]));
            }
        }
        farben_angleichen(&mut erzeugt, &vorlage, &|_, _| true);
        let himmel = erzeugt.get_pixel(40, 40);
        for c in 0..3 {
            assert!((himmel[c] as i32 - vorlage.get_pixel(40, 40)[c] as i32).abs() <= 2, "{himmel:?}");
        }
    }

    #[test]
    fn kasten_hat_umgebung_und_bleibt_im_bild() {
        let k = kasten_um(&[[0.5, 0.2, 0.05]], 1.5);
        assert!(k[0] < 0.45 && k[2] > 0.55);
        assert!(k.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
