//! Objekte entfernen mit KI-Fuellung — das lokale Retusche-Modell.
//!
//! ## Das Verfahren
//! Ein Inpainting-Netz (LaMa, Apache-2.0, ONNX) erfindet die markierte Stelle aus
//! ihrer Umgebung neu, statt wie der klassische Pinsel (`heal`) eine passende
//! Stelle zu kopieren. Das traegt auch dort, wo es keine Kopiervorlage gibt:
//! eine Person vor einer Wand, ein Mast vor dem Himmel, ein Schild im Rasen.
//!
//! 1. **Bereiche bilden.** Ueberlappende Flecken werden zu einem Bereich
//!    zusammengefasst; jeder Bereich wird einzeln gefuellt.
//! 2. **Ausschnitt mit Umgebung.** Um den Bereich herum wird ein quadratischer
//!    Ausschnitt genommen (dreimal so gross wie die Stelle, mindestens 256 px),
//!    damit das Netz genug Umgebung sieht. Das Netz rechnet fest mit 512 x 512.
//! 3. **Farbraum.** Gerechnet wird hier auf linearen Kameradaten. Das Netz
//!    kennt nur fertige Bilder — deshalb wird der Ausschnitt auf seine hellen
//!    Werte normiert und mit einer Gammakurve gestaucht, gefuellt und exakt
//!    zurueckgerechnet.
//! 4. **Maske weiten.** Die Maske wird etwas ueber den markierten Rand hinaus
//!    geweitet. Ohne das bleibt ein Saum des Objekts stehen, aus dem das Netz
//!    einen „Geist" des Objekts rekonstruiert (gemessen am Beispiel des Modells).
//! 5. **Weich einsetzen.** Nur der Bereich wird ersetzt, der Rand laeuft weich aus.
//!
//! Laufzeit: reines Rust (`tract`), etwa 1,5 s je Bereich auf der CPU. Die
//! Stellen werden einmal beim Laden geschlossen, nicht bei jedem Reglerzug.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tract_onnx::prelude::*;

use crate::heal::Fleck;

type Netz = std::sync::Arc<TypedRunnableModel>;

/// Seitenlaenge, mit der das Netz rechnet (fest exportiert).
const N: usize = 512;
/// Gamma fuer die Umrechnung linear -> „fertiges Bild".
const GAMMA: f32 = 2.2;

static MODELL_PFAD: Mutex<Option<PathBuf>> = Mutex::new(None);
static NETZ: OnceLock<Mutex<Option<(PathBuf, Netz)>>> = OnceLock::new();

/// Wo das Modell liegt (`None` = nicht installiert). Setzt die Anwendung beim
/// Start und nach dem Installieren.
pub fn modell_setzen(pfad: Option<PathBuf>) {
    if let Ok(mut p) = MODELL_PFAD.lock() {
        *p = pfad;
    }
}

/// Ist ein Modell eingestellt und vorhanden?
pub fn verfuegbar() -> bool {
    MODELL_PFAD
        .lock()
        .ok()
        .and_then(|p| p.clone())
        .is_some_and(|p| p.is_file())
}

fn netz() -> Result<Netz, String> {
    let pfad = MODELL_PFAD
        .lock()
        .ok()
        .and_then(|p| p.clone())
        .filter(|p| p.is_file())
        .ok_or_else(|| "Retusche-Modell nicht installiert".to_string())?;
    let zelle = NETZ.get_or_init(|| Mutex::new(None));
    let mut g = zelle.lock().map_err(|_| "Retusche-Modell gesperrt".to_string())?;
    if let Some((p, n)) = g.as_ref() {
        if *p == pfad {
            return Ok(n.clone());
        }
    }
    let n = laden(&pfad).map_err(|e| e.to_string())?;
    *g = Some((pfad, n.clone()));
    Ok(n)
}

fn laden(pfad: &Path) -> TractResult<Netz> {
    tract_onnx::onnx()
        .model_for_path(pfad)?
        .with_input_fact(0, f32::fact([1, 3, N, N]).into())?
        .with_input_fact(1, f32::fact([1, 1, N, N]).into())?
        .into_optimized()?
        .into_runnable()
}

/// Ein zusammenhaengender Bereich aus Flecken.
#[derive(Debug, Clone)]
struct Bereich {
    flecken: Vec<Fleck>,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

fn bereiche(flecken: &[Fleck]) -> Vec<Bereich> {
    let mut out: Vec<Bereich> = Vec::new();
    for f in flecken {
        let (x0, y0, x1, y1) = (f.x - f.r, f.y - f.r, f.x + f.r, f.y + f.r);
        // Mit allen Bereichen verschmelzen, die sich (mit etwas Abstand) beruehren.
        let rand = f.r;
        let mut neu = Bereich { flecken: vec![*f], x0, y0, x1, y1 };
        let mut i = 0;
        while i < out.len() {
            let b = &out[i];
            let beruehrt = b.x0 - rand <= neu.x1 && neu.x0 <= b.x1 + rand && b.y0 - rand <= neu.y1 && neu.y0 <= b.y1 + rand;
            if beruehrt {
                let b = out.swap_remove(i);
                neu.flecken.extend(b.flecken);
                neu.x0 = neu.x0.min(b.x0);
                neu.y0 = neu.y0.min(b.y0);
                neu.x1 = neu.x1.max(b.x1);
                neu.y1 = neu.y1.max(b.y1);
                i = 0;
            } else {
                i += 1;
            }
        }
        out.push(neu);
    }
    out
}

/// Fuellt alle Flecken mit dem Netz. Gibt `Err` zurueck, wenn das Modell fehlt
/// oder nicht rechnet — der Aufrufer faellt dann auf den klassischen Pinsel zurueck.
pub fn anwenden(data: &mut [u16], w: u32, h: u32, flecken: &[Fleck]) -> Result<(), String> {
    if flecken.is_empty() {
        return Ok(());
    }
    let netz = netz()?;
    for b in bereiche(flecken) {
        einen_bereich(&netz, data, w, h, &b)?;
    }
    Ok(())
}

fn einen_bereich(netz: &Netz, data: &mut [u16], w: u32, h: u32, b: &Bereich) -> Result<(), String> {
    let (bw, bh) = (b.x1 - b.x0, b.y1 - b.y0);
    // Quadratischer Ausschnitt: dreimal die Stelle, mindestens 256 px, hoechstens das Bild.
    let seite = (bw.max(bh) * 3.0).max(256.0).min(w.min(h) as f32).round() as i64;
    let cx = (b.x0 + b.x1) / 2.0;
    let cy = (b.y0 + b.y1) / 2.0;
    let ax = ((cx - seite as f32 / 2.0).round() as i64).clamp(0, w as i64 - seite);
    let ay = ((cy - seite as f32 / 2.0).round() as i64).clamp(0, h as i64 - seite);
    let s = seite as f32 / N as f32; // Ausschnitt-Pixel je Netz-Pixel

    // Maske im Netz-Raster: Flecken + Weitung (gemessen: ~8 Netz-Pixel genuegen,
    // damit kein Saum des Objekts stehen bleibt).
    let weite = 8.0f32;
    let mut maske = vec![0.0f32; N * N];
    for y in 0..N {
        for x in 0..N {
            let px = ax as f32 + (x as f32 + 0.5) * s;
            let py = ay as f32 + (y as f32 + 0.5) * s;
            let drin = b.flecken.iter().any(|f| {
                let d = ((px - f.x).powi(2) + (py - f.y).powi(2)).sqrt();
                d <= f.r + weite * s
            });
            if drin {
                maske[y * N + x] = 1.0;
            }
        }
    }

    // Ausschnitt abtasten (bilinear) — linear, RGB.
    let lese = |x: f32, y: f32, c: usize| -> f32 {
        let x = x.clamp(0.0, w as f32 - 1.0);
        let y = y.clamp(0.0, h as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w as usize - 1), (y0 + 1).min(h as usize - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| half::f16::from_bits(data[(yy * w as usize + xx) * 4 + c]).to_f32();
        let o = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
        let u = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
        o * (1.0 - fy) + u * fy
    };
    let mut lin = vec![0.0f32; 3 * N * N];
    for y in 0..N {
        for x in 0..N {
            let px = ax as f32 + (x as f32 + 0.5) * s - 0.5;
            let py = ay as f32 + (y as f32 + 0.5) * s - 0.5;
            for c in 0..3 {
                lin[c * N * N + y * N + x] = lese(px, py, c).max(0.0);
            }
        }
    }
    // Normierung auf die hellen Werte der UMGEBUNG (nicht des Objekts).
    let mut umgebung: Vec<f32> = (0..N * N)
        .filter(|&i| maske[i] < 0.5)
        .map(|i| lin[i].max(lin[N * N + i]).max(lin[2 * N * N + i]))
        .collect();
    if umgebung.is_empty() {
        return Ok(());
    }
    umgebung.sort_by(|a, b| a.total_cmp(b));
    let weiss = umgebung[(umgebung.len() as f32 * 0.995) as usize].max(1e-4);
    let ein: Tensor = tract_ndarray::Array4::from_shape_fn((1, 3, N, N), |(_, c, y, x)| {
        (lin[c * N * N + y * N + x] / weiss).clamp(0.0, 1.0).powf(1.0 / GAMMA)
    })
    .into();
    let mk: Tensor = tract_ndarray::Array4::from_shape_fn((1, 1, N, N), |(_, _, y, x)| maske[y * N + x]).into();
    let aus = netz.run(tvec!(ein.into(), mk.into())).map_err(|e| e.to_string())?;
    let t = aus[0].clone().into_tensor();
    let o = t.to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
    // Ausgabe 0..255 (gemessen), zurueck in linear und auf die alte Skala.
    let netz_px = |c: usize, x: f32, y: f32| -> f32 {
        let x = x.clamp(0.0, N as f32 - 1.0);
        let y = y.clamp(0.0, N as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(N - 1), (y0 + 1).min(N - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let v = |xx: usize, yy: usize| (o[[0, c, yy, xx]] / 255.0).clamp(0.0, 1.0).powf(GAMMA) * weiss;
        let ob = v(x0, y0) * (1.0 - fx) + v(x1, y0) * fx;
        let un = v(x0, y1) * (1.0 - fx) + v(x1, y1) * fx;
        ob * (1.0 - fy) + un * fy
    };
    // Einsetzen: innerhalb der Flecken voll, weicher Rand ueber die Weitung.
    let rand = (weite * s).max(2.0);
    for yy in ay..ay + seite {
        for xx in ax..ax + seite {
            let (px, py) = (xx as f32 + 0.5, yy as f32 + 0.5);
            let mut a = 0.0f32;
            for f in &b.flecken {
                let d = ((px - f.x).powi(2) + (py - f.y).powi(2)).sqrt();
                let t = ((f.r + rand - d) / rand).clamp(0.0, 1.0);
                a = a.max(t * t * (3.0 - 2.0 * t));
            }
            if a <= 0.0 {
                continue;
            }
            let nx = (xx - ax) as f32 / s - 0.5;
            let ny = (yy - ay) as f32 / s - 0.5;
            let i = (yy as usize * w as usize + xx as usize) * 4;
            for c in 0..3 {
                let alt = half::f16::from_bits(data[i + c]).to_f32();
                let neu = netz_px(c, nx, ny);
                data[i + c] = half::f16::from_f32(alt * (1.0 - a) + neu * a).to_bits();
            }
        }
    }
    Ok(())
}

/// Fuellt leere Flaechen eines Bildes (z. B. die ausgefransten Raender eines
/// Panoramas) mit dem Retusche-Netz. `rgb` ist linear, verschraenkt RGB,
/// `leer[i] != 0` markiert fehlende Pixel; gefuellte Pixel werden in `leer`
/// auf 0 gesetzt. Gerechnet wird kachelweise entlang der leeren Flaechen —
/// jede gefuellte Kachel ist Umgebung fuer die naechste. Gibt die Zahl der
/// gefuellten Pixel zurueck.
pub fn flaechen_fuellen(rgb: &mut [f32], w: u32, h: u32, leer: &mut [u8], fortschritt: &dyn Fn(f32)) -> Result<usize, String> {
    // Grosse Bilder erst im Ganzen auf kleiner Fassung fuellen: kachelweise auf
    // voller Aufloesung sah eine grosse leere Ecke in jeder Kachel zu wenig
    // Umgebung — dunkle Schlieren und harte Kachelkanten im Himmel (gemessen).
    // Auf ~1024 px sieht jede Kachel einen grossen Teil des Panoramas; die
    // Fuellung wird fuer die leeren Pixel hochgerechnet (Himmel, Wasser, Boden
    // an den Raendern vertragen das).
    const KLEIN: usize = 1024;
    let (wu, hu) = (w as usize, h as usize);
    if wu.max(hu) > KLEIN * 3 / 2 {
        let f = KLEIN as f32 / wu.max(hu) as f32;
        let (kw, kh) = (((wu as f32 * f).round() as usize).max(8), ((hu as f32 * f).round() as usize).max(8));
        let (sx, sy) = (wu as f32 / kw as f32, hu as f32 / kh as f32);
        let mut klein = vec![0f32; kw * kh * 3];
        let mut klein_leer = vec![0u8; kw * kh];
        for y in 0..kh {
            for x in 0..kw {
                let (x0, x1) = ((x as f32 * sx) as usize, (((x + 1) as f32 * sx) as usize).min(wu).max((x as f32 * sx) as usize + 1));
                let (y0, y1) = ((y as f32 * sy) as usize, (((y + 1) as f32 * sy) as usize).min(hu).max((y as f32 * sy) as usize + 1));
                let (mut s3, mut n, mut n_leer) = ([0f32; 3], 0f32, 0usize);
                for yy in y0..y1 {
                    for xx in x0..x1 {
                        let i = yy * wu + xx;
                        if leer[i] != 0 {
                            n_leer += 1;
                            continue;
                        }
                        for c in 0..3 {
                            s3[c] += rgb[i * 3 + c];
                        }
                        n += 1.0;
                    }
                }
                let k = y * kw + x;
                if n_leer * 2 > (x1 - x0) * (y1 - y0) || n == 0.0 {
                    klein_leer[k] = 1;
                } else {
                    for c in 0..3 {
                        klein[k * 3 + c] = s3[c] / n;
                    }
                }
            }
        }
        flaechen_fuellen(&mut klein, kw as u32, kh as u32, &mut klein_leer, fortschritt)?;
        // Leere Pixel aus der kleinen, gefuellten Fassung (bilinear).
        let mut gefuellt = 0usize;
        for y in 0..hu {
            for x in 0..wu {
                let i = y * wu + x;
                if leer[i] == 0 {
                    continue;
                }
                let fx = ((x as f32 + 0.5) / sx - 0.5).clamp(0.0, kw as f32 - 1.0);
                let fy = ((y as f32 + 0.5) / sy - 0.5).clamp(0.0, kh as f32 - 1.0);
                let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(kw - 1), (y0 + 1).min(kh - 1));
                let (ax, ay) = (fx - x0 as f32, fy - y0 as f32);
                for c in 0..3 {
                    let v = |xx: usize, yy: usize| klein[(yy * kw + xx) * 3 + c];
                    rgb[i * 3 + c] = (v(x0, y0) * (1.0 - ax) + v(x1, y0) * ax) * (1.0 - ay) + (v(x0, y1) * (1.0 - ax) + v(x1, y1) * ax) * ay;
                }
                leer[i] = 0;
                gefuellt += 1;
            }
        }
        return Ok(gefuellt);
    }
    let netz = netz()?;
    let (w, h) = (w as usize, h as usize);
    let gesamt = leer.iter().filter(|v| **v != 0).count();
    if gesamt == 0 {
        return Ok(0);
    }
    // Kachelgroesse: gross genug fuer Umgebung, klein genug fuer Schaerfe.
    let seite = (w.min(h) / 2).clamp(256, 1536);
    let schritt = seite / 2;
    let mut gefuellt = 0usize;
    // Mehrere Durchgaenge: tief in leere Ecken hinein braucht es Umgebung, die
    // erst ein frueherer Durchgang geschaffen hat.
    for durchgang in 0..6 {
        let mut etwas = false;
        let mut y0 = 0usize;
        while y0 < h {
            let mut x0 = 0usize;
            while x0 < w {
                let (x1, y1) = ((x0 + seite).min(w), (y0 + seite).min(h));
                let (kx0, ky0) = (x1.saturating_sub(seite), y1.saturating_sub(seite));
                let (kw, kh) = (x1 - kx0, y1 - ky0);
                let mut n_leer = 0usize;
                for y in ky0..y1 {
                    n_leer += leer[y * w + kx0..y * w + x1].iter().filter(|v| **v != 0).count();
                }
                let anteil = n_leer as f32 / (kw * kh) as f32;
                // Nur Kacheln mit Luecke UND genug Bild drumherum.
                if n_leer > 0 && anteil < 0.7 {
                    kachel(&netz, rgb, w, leer, kx0, ky0, kw, kh)?;
                    gefuellt += n_leer;
                    etwas = true;
                    fortschritt((gefuellt as f32 / gesamt as f32).min(1.0));
                }
                if x1 == w {
                    break;
                }
                x0 += schritt;
            }
            if y1_ende(y0, seite, h) {
                break;
            }
            y0 += schritt;
        }
        if !etwas || leer.iter().all(|v| *v == 0) {
            let _ = durchgang;
            break;
        }
    }
    Ok(gefuellt)
}

fn y1_ende(y0: usize, seite: usize, h: usize) -> bool {
    (y0 + seite).min(h) == h
}

#[allow(clippy::too_many_arguments)]
fn kachel(netz: &Netz, rgb: &mut [f32], w: usize, leer: &mut [u8], kx0: usize, ky0: usize, kw: usize, kh: usize) -> Result<(), String> {
    let sx = kw as f32 / N as f32;
    let sy = kh as f32 / N as f32;
    let px = |x: f32, y: f32, c: usize| -> f32 {
        let xi = (kx0 as f32 + x * sx) as usize;
        let yi = (ky0 as f32 + y * sy) as usize;
        rgb[(yi.min(kh + ky0 - 1) * w + xi.min(kw + kx0 - 1)) * 3 + c]
    };
    let ist_leer = |x: f32, y: f32| -> bool {
        let xi = ((kx0 as f32 + x * sx) as usize).min(kx0 + kw - 1);
        let yi = ((ky0 as f32 + y * sy) as usize).min(ky0 + kh - 1);
        leer[yi * w + xi] != 0
    };
    // Maske im Netz-Raster, etwas geweitet (Saum an der Kante).
    let mut maske = vec![0.0f32; N * N];
    for y in 0..N {
        for x in 0..N {
            if ist_leer(x as f32 + 0.5, y as f32 + 0.5) {
                for dy in -3i32..=3 {
                    for dx in -3i32..=3 {
                        let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                        if (0..N as i32).contains(&xx) && (0..N as i32).contains(&yy) {
                            maske[yy as usize * N + xx as usize] = 1.0;
                        }
                    }
                }
            }
        }
    }
    let mut hell: Vec<f32> = (0..N * N)
        .filter(|&i| maske[i] < 0.5)
        .map(|i| {
            let (x, y) = ((i % N) as f32 + 0.5, (i / N) as f32 + 0.5);
            px(x, y, 0).max(px(x, y, 1)).max(px(x, y, 2))
        })
        .collect();
    if hell.len() < 100 {
        return Ok(());
    }
    hell.sort_by(|a, b| a.total_cmp(b));
    let weiss = hell[(hell.len() as f32 * 0.995) as usize].max(1e-4);
    let ein: Tensor = tract_ndarray::Array4::from_shape_fn((1, 3, N, N), |(_, c, y, x)| {
        if maske[y * N + x] > 0.5 {
            0.0
        } else {
            (px(x as f32 + 0.5, y as f32 + 0.5, c) / weiss).clamp(0.0, 1.0).powf(1.0 / GAMMA)
        }
    })
    .into();
    let mk: Tensor = tract_ndarray::Array4::from_shape_fn((1, 1, N, N), |(_, _, y, x)| maske[y * N + x]).into();
    let aus = netz.run(tvec!(ein.into(), mk.into())).map_err(|e| e.to_string())?;
    let t = aus[0].clone().into_tensor();
    let o = t.to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
    let lese = |c: usize, x: f32, y: f32| -> f32 {
        let x = x.clamp(0.0, N as f32 - 1.0);
        let y = y.clamp(0.0, N as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(N - 1), (y0 + 1).min(N - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let v = |xx: usize, yy: usize| (o[[0, c, yy, xx]] / 255.0).clamp(0.0, 1.0).powf(GAMMA) * weiss;
        (v(x0, y0) * (1.0 - fx) + v(x1, y0) * fx) * (1.0 - fy) + (v(x0, y1) * (1.0 - fx) + v(x1, y1) * fx) * fy
    };
    // Nur die leeren Pixel ersetzen — vorhandene Bilddaten bleiben unangetastet.
    for y in ky0..ky0 + kh {
        for x in kx0..kx0 + kw {
            let i = y * w + x;
            if leer[i] == 0 {
                continue;
            }
            let nx = (x - kx0) as f32 / sx - 0.5;
            let ny = (y - ky0) as f32 / sy - 0.5;
            for c in 0..3 {
                rgb[i * 3 + c] = lese(c, nx, ny);
            }
            leer[i] = 0;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beruehrende_flecken_werden_ein_bereich() {
        let f = |x, y| Fleck { x, y, r: 10.0 };
        let b = bereiche(&[f(100.0, 100.0), f(115.0, 100.0), f(400.0, 400.0)]);
        assert_eq!(b.len(), 2);
        assert!(b.iter().any(|b| b.flecken.len() == 2));
    }

    #[test]
    fn ohne_modell_meldet_es_einen_fehler() {
        modell_setzen(Some(PathBuf::from("/gibt/es/nicht.onnx")));
        let mut data = vec![0u16; 64 * 64 * 4];
        let r = anwenden(&mut data, 64, 64, &[Fleck { x: 10.0, y: 10.0, r: 4.0 }]);
        assert!(r.is_err());
        assert!(!verfuegbar());
        modell_setzen(None);
    }
}
