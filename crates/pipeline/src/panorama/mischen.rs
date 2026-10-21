//! Verzerren und Mehrband-Mischung auf voller Aufloesung.
//!
//! ## Warum Mehrband (Laplace-Pyramide)
//! Eine harte Naht zeigt jeden Rest-Helligkeitsunterschied als Kante; eine breite
//! weiche Ueberblendung verdoppelt dagegen alles, was nicht exakt deckt (Blaetter,
//! Wellen, Menschen). Die Pyramide trennt beides: feine Details wechseln fast hart
//! an der Naht (keine Doppelbilder), grobe Helligkeitsverlaeufe ueber Hunderte
//! Pixel (keine sichtbare Stufe).
//!
//! ## Warum in Wurzel-Helligkeit
//! Gemischt wird auf sqrt(linear), am Ende wird zurueck quadriert. Wo nur ein Bild
//! liegt, kommt so exakt das lineare Original heraus; in den Uebergaengen verhaelt
//! sich die Mischung aber wahrnehmungsnaeher — in rein linearen Daten erzeugt die
//! Pyramide an hellen Kanten dunkle Saeume in den Schatten.
//!
//! ## Speicher
//! Bilder werden NACHEINANDER verzerrt und in eine gemeinsame Pyramide addiert;
//! gleichzeitig existiert nur die Pyramide EINES verzerrten Bildes. Die Ausgabe ist
//! die unterste Stufe der Summenpyramide selbst — keine weitere Vollkopie.
//!
//! ## Raender
//! Jede Pyramidenstufe eines Bildes waechst um ein bis zwei Pixel ueber ihr
//! Rechteck hinaus (Bild randverlaengert, Gewicht null), die Stufen der Leinwand
//! haben 2 px Rand. Ohne das endet die Summenpyramide genau am Bildrand, und beim
//! Wiederaufbau mischt das Vergroessern die Nullen dahinter in die Randpixel —
//! ein dunkler Saum entlang jeder Bildkante und am Panoramarand (gemessen: bis
//! zur halben Helligkeit).

use super::naht::abtasten;
use super::projektion::Leinwand;
use rayon::prelude::*;

const K5: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Eine Pyramidenstufe: Ausschnitt [ax, ax + w) x [ay, ay + h) in
/// Leinwandkoordinaten dieser Stufe, `ch` Kanaele verschraenkt.
pub(super) struct Ebene {
    pub d: Vec<f32>,
    pub ax: isize,
    pub ay: isize,
    pub w: usize,
    pub h: usize,
}

/// Halbieren mit Binomialfilter. Die Ausgabe reicht ein Pixel ueber das
/// Halbierte hinaus; ausserhalb der Eingabe gilt Randverlaengerung
/// (`null_rand = false`, fuer Bilddaten) oder null (fuer Gewichte).
fn halbieren(e: &Ebene, ch: usize, null_rand: bool) -> Ebene {
    let ax2 = e.ax.div_euclid(2) - 1;
    let ay2 = e.ay.div_euclid(2) - 1;
    let bx2 = (e.ax + e.w as isize + 1).div_euclid(2) + 1;
    let by2 = (e.ay + e.h as isize + 1).div_euclid(2) + 1;
    let (w2, h2) = ((bx2 - ax2) as usize, (by2 - ay2) as usize);
    let (w, h) = (e.w as isize, e.h as isize);
    let mut t = vec![0.0f32; w2 * e.h * ch];
    t.par_chunks_mut(w2 * ch).enumerate().for_each(|(y, z)| {
        let ein = &e.d[y * e.w * ch..(y + 1) * e.w * ch];
        for x2 in 0..w2 {
            let mitte = 2 * (ax2 + x2 as isize) - e.ax;
            for (o, k) in K5.iter().enumerate() {
                let mut x = mitte + o as isize - 2;
                if x < 0 || x >= w {
                    if null_rand {
                        continue;
                    }
                    x = x.clamp(0, w - 1);
                }
                let x = x as usize;
                for c in 0..ch {
                    z[x2 * ch + c] += ein[x * ch + c] * k;
                }
            }
        }
    });
    let mut d = vec![0.0f32; w2 * h2 * ch];
    d.par_chunks_mut(w2 * ch).enumerate().for_each(|(y2, z)| {
        let mitte = 2 * (ay2 + y2 as isize) - e.ay;
        for (o, k) in K5.iter().enumerate() {
            let mut y = mitte + o as isize - 2;
            if y < 0 || y >= h {
                if null_rand {
                    continue;
                }
                y = y.clamp(0, h - 1);
            }
            let y = y as usize;
            let ein = &t[y * w2 * ch..(y + 1) * w2 * ch];
            for (a, v) in z.iter_mut().zip(ein) {
                *a += v * k;
            }
        }
    });
    Ebene { d, ax: ax2, ay: ay2, w: w2, h: h2 }
}

/// Verdoppeln der Stufe `g` auf die naechstfeinere Stufe `ziel` und dort
/// abziehen (Laplace-Stufe bilden) oder addieren (Wiederaufbau) — passend zum
/// Halbieren (Kern 2 * Binomial auf null-eingefuegtem Gitter): gerade Stellen
/// (1 6 1)/8, ungerade (1 1)/2.
///
/// Zeilenweise direkt in das Ziel: eine Zwischenkopie in Zielgroesse waere auf
/// der untersten Leinwandstufe so gross wie das ganze Panorama (bei 100 MP
/// ueber 1 GB). Jede Ausgabezeile rechnet ihre zwei bis drei groben Zeilen
/// selbst waagrecht hoch — etwas Mehrarbeit, kaum Speicher.
fn verdoppeln_in(g: &Ebene, ch: usize, ziel: &mut Ebene, abziehen: bool) {
    let gw = g.w as isize;
    let gh = g.h as isize;
    let (ax, ay, w) = (ziel.ax, ziel.ay, ziel.w);
    let ix = |j: isize| (j - g.ax).clamp(0, gw - 1) as usize;
    let iy = |j: isize| (j - g.ay).clamp(0, gh - 1) as usize;
    // Waagrechte Indizes und Gewichte einmal vorab.
    let spalten: Vec<([usize; 3], [f32; 3])> = (0..w)
        .map(|x| {
            let i = ax + x as isize;
            let j = i.div_euclid(2);
            if i.rem_euclid(2) == 0 {
                ([ix(j - 1), ix(j), ix(j + 1)], [0.125, 0.75, 0.125])
            } else {
                ([ix(j), ix(j + 1), ix(j + 1)], [0.5, 0.5, 0.0])
            }
        })
        .collect();
    let zeile_hoch = |r: usize, aus: &mut [f32]| {
        let ein = &g.d[r * g.w * ch..(r + 1) * g.w * ch];
        for (x, (idx, gew)) in spalten.iter().enumerate() {
            for c in 0..ch {
                aus[x * ch + c] = ein[idx[0] * ch + c] * gew[0] + ein[idx[1] * ch + c] * gew[1] + ein[idx[2] * ch + c] * gew[2];
            }
        }
    };
    let vz = if abziehen { -1.0f32 } else { 1.0 };
    ziel.d.par_chunks_mut(w * ch).enumerate().for_each_init(
        || (vec![0.0f32; w * ch], vec![0.0f32; w * ch], vec![0.0f32; w * ch]),
        |(za, zb, zc), (y, z)| {
            let i = ay + y as isize;
            let j = i.div_euclid(2);
            if i.rem_euclid(2) == 0 {
                zeile_hoch(iy(j - 1), za);
                zeile_hoch(iy(j), zb);
                zeile_hoch(iy(j + 1), zc);
                for k in 0..w * ch {
                    z[k] += vz * (za[k] + 6.0 * zb[k] + zc[k]) * 0.125;
                }
            } else {
                zeile_hoch(iy(j), za);
                zeile_hoch(iy(j + 1), zb);
                for k in 0..w * ch {
                    z[k] += vz * (za[k] + zb[k]) * 0.5;
                }
            }
        },
    );
}

/// Zahl der Pyramidenstufen wie in der Referenzumsetzung von Brown & Lowe
/// (Beispielablauf): blend_width = sqrt(Flaeche) · 5 %, Stufen =
/// ceil(log2(blend_width)) − 1; hoechstens so viele, dass die groebste Stufe
/// noch zwei Pixel hat.
pub fn stufen(w: usize, h: usize) -> usize {
    let breite = 0.05 * ((w * h) as f64).sqrt();
    let wunsch = if breite < 1.0 { 0 } else { (breite.log2().ceil() as isize - 1).max(0) as usize };
    let grenze = (w.max(h) as f64).log2().ceil() as usize;
    wunsch.min(grenze).max(1)
}

/// Quelle eines Bildes fuers Mischen (volle Aufloesung).
pub struct Quelle<'a> {
    pub rgb: &'a [f32],
    pub w: usize,
    pub h: usize,
    pub gain: f32,
    /// Leinwandstelle (volle Leinwand, stetig) -> Quellkoordinate im Bild.
    pub abbildung: &'a (dyn Fn(f64, f64) -> Option<(f64, f64)> + Sync),
    /// Rechteck [x0, x1) x [y0, y1) auf der Leinwand, das das Bild abdeckt.
    pub rechteck: (usize, usize, usize, usize),
}

/// Addiert `wert * gewicht` (bzw. das Gewicht) einer Bildstufe in die Leinwandstufe.
fn aufaddieren(acc: &mut Ebene, wacc: &mut Ebene, wert: &Ebene, gew: &Ebene) {
    let cw = acc.w;
    let (ax, ay) = (acc.ax, acc.ay);
    acc.d.par_chunks_mut(cw * 3).zip(wacc.d.par_chunks_mut(cw)).enumerate().for_each(|(zeile, (za, zw))| {
        let y = ay + zeile as isize;
        let t = y - wert.ay;
        if t < 0 || t >= wert.h as isize {
            return;
        }
        let t = t as usize;
        let zl = &wert.d[t * wert.w * 3..(t + 1) * wert.w * 3];
        let zg = &gew.d[t * gew.w..(t + 1) * gew.w];
        let x_von = wert.ax.max(ax);
        let x_bis = (wert.ax + wert.w as isize).min(ax + cw as isize);
        for x in x_von..x_bis {
            let s = (x - wert.ax) as usize;
            let g = zg[s];
            if g == 0.0 {
                continue;
            }
            let o = (x - ax) as usize;
            zw[o] += g;
            for c in 0..3 {
                za[o * 3 + c] += zl[s * 3 + c] * g;
            }
        }
    });
}

/// Mischt alle Bilder auf die Leinwand. `label`/`lw`/`lh`: Aufteilung auf der
/// kleinen Leinwand im Massstab `sigma` (klein = gross * sigma).
pub fn mischen(
    l: &Leinwand,
    quellen: &[Quelle],
    label: &[u16],
    lw: usize,
    lh: usize,
    sigma: f64,
    fortschritt: &dyn Fn(f32),
) -> (Vec<f32>, Vec<u8>) {
    let (w, h) = (l.w, l.h);
    let n_st = stufen(w, h);
    // Leinwandstufen; ab Stufe 1 mit 2 px Rand (siehe Modulkopf).
    let mut acc: Vec<Ebene> = Vec::new();
    let mut wacc: Vec<Ebene> = Vec::new();
    let (mut sw, mut sh) = (w, h);
    for k in 0..=n_st {
        if k > 0 {
            sw = sw.div_ceil(2);
            sh = sh.div_ceil(2);
        }
        let rand = if k == 0 { 0 } else { 2 };
        let (ew, eh) = (sw + 2 * rand, sh + 2 * rand);
        acc.push(Ebene { d: vec![0.0; ew * eh * 3], ax: -(rand as isize), ay: -(rand as isize), w: ew, h: eh });
        wacc.push(Ebene { d: vec![0.0; ew * eh], ax: -(rand as isize), ay: -(rand as isize), w: ew, h: eh });
    }
    let mut deckung = vec![0u8; w * h];

    for (idx, q) in quellen.iter().enumerate() {
        fortschritt(idx as f32 / quellen.len() as f32);
        let (x0, x1, y0, y1) = q.rechteck;
        let (x1, y1) = (x1.min(w), y1.min(h));
        if x0 >= x1 || y0 >= y1 {
            continue;
        }
        let (rw, rh) = (x1 - x0, y1 - y0);
        let mut bild = vec![0.0f32; rw * rh * 3];
        let mut gew = vec![0.0f32; rw * rh];
        let lab = idx as u16;
        bild.par_chunks_mut(rw * 3).zip(gew.par_chunks_mut(rw)).enumerate().for_each(|(yy, (zb, zg))| {
            let y = y0 + yy;
            let sy = ((y as f64 + 0.5) * sigma - 0.5).clamp(0.0, (lh - 1) as f64);
            let ly0 = sy.floor() as usize;
            let ly1 = (ly0 + 1).min(lh - 1);
            let ay = (sy - ly0 as f64) as f32;
            for xx in 0..rw {
                let x = x0 + xx;
                let Some((qx, qy)) = (q.abbildung)(x as f64 + 0.5, y as f64 + 0.5) else { continue };
                // Auch ausserhalb des Bildes randverlaengert abtasten: die Pyramide
                // sieht dann keine harte Kante zu Schwarz.
                let p = abtasten(q.rgb, q.w, q.h, qx as f32, qy as f32);
                for c in 0..3 {
                    zb[xx * 3 + c] = (p[c] * q.gain).max(0.0).sqrt();
                }
                if qx < 0.5 || qy < 0.5 || qx > q.w as f64 - 0.5 || qy > q.h as f64 - 0.5 {
                    continue;
                }
                let sx = ((x as f64 + 0.5) * sigma - 0.5).clamp(0.0, (lw - 1) as f64);
                let lx0 = sx.floor() as usize;
                let lx1 = (lx0 + 1).min(lw - 1);
                let ax = (sx - lx0 as f64) as f32;
                let ist = |i: usize| if label[i] == lab { 1.0f32 } else { 0.0 };
                let a = ist(ly0 * lw + lx0) * (1.0 - ax) + ist(ly0 * lw + lx1) * ax;
                let b = ist(ly1 * lw + lx0) * (1.0 - ax) + ist(ly1 * lw + lx1) * ax;
                // Winziger Grundanteil: wo die grobe Aufteilung (noch) kein Bild
                // vorsieht, aber Daten liegen (Bildraender), bleibt kein Loch.
                zg[xx] = a + (b - a) * ay + 1e-4;
            }
        });
        deckung.par_chunks_mut(w).skip(y0).take(rh).zip(gew.par_chunks(rw)).for_each(|(zd, zg)| {
            for (d, g) in zd[x0..x0 + rw].iter_mut().zip(zg) {
                if *g > 0.0 {
                    *d = 255;
                }
            }
        });
        // Gauss-Pyramide des Bildes und der Gewichte.
        let mut gb = vec![Ebene { d: bild, ax: x0 as isize, ay: y0 as isize, w: rw, h: rh }];
        let mut gg = vec![Ebene { d: gew, ax: x0 as isize, ay: y0 as isize, w: rw, h: rh }];
        for k in 1..=n_st {
            let n = halbieren(&gb[k - 1], 3, false);
            let m = halbieren(&gg[k - 1], 1, true);
            gb.push(n);
            gg.push(m);
        }
        // Laplace-Stufen bilden und gewichtet aufsummieren.
        for k in 0..=n_st {
            let mut lap = std::mem::replace(&mut gb[k], Ebene { d: vec![], ax: 0, ay: 0, w: 0, h: 0 });
            if k < n_st {
                verdoppeln_in(&gb[k + 1], 3, &mut lap, true);
            }
            aufaddieren(&mut acc[k], &mut wacc[k], &lap, &gg[k]);
            gg[k].d = Vec::new();
        }
    }

    // Jede Stufe durch ihr Gesamtgewicht teilen, dann von grob nach fein aufbauen.
    for k in 0..=n_st {
        let wk = std::mem::take(&mut wacc[k].d);
        acc[k].d.par_chunks_mut(3).zip(wk.par_iter()).for_each(|(p, g)| {
            let i = if *g > 1e-12 { 1.0 / g } else { 0.0 };
            p[0] *= i;
            p[1] *= i;
            p[2] *= i;
        });
    }
    for k in (0..n_st).rev() {
        let grob = std::mem::replace(&mut acc[k + 1], Ebene { d: vec![], ax: 0, ay: 0, w: 0, h: 0 });
        verdoppeln_in(&grob, 3, &mut acc[k], false);
    }
    let mut aus = std::mem::take(&mut acc[0].d);
    aus.par_chunks_mut(3).zip(deckung.par_iter()).for_each(|(p, d)| {
        for v in p.iter_mut() {
            *v = if *d == 0 { 0.0 } else { v.max(0.0) * v.max(0.0) };
        }
    });
    (aus, deckung)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyramide_erhaelt_flaechen() {
        // Gleichfarbige Flaeche bleibt beim Halbieren und Verdoppeln exakt gleich
        // (Kerne normiert, Raender verlaengert) — sonst entstuenden Helligkeitsringe.
        let (w, h) = (37usize, 23usize);
        let e = Ebene { d: vec![0.4f32; w * h * 3], ax: 5, ay: -3, w, h };
        let g = halbieren(&e, 3, false);
        assert!(g.d.iter().all(|v| (v - 0.4).abs() < 1e-6));
        let mut hoch = Ebene { d: vec![0.0; w * h * 3], ax: e.ax, ay: e.ay, w, h };
        verdoppeln_in(&g, 3, &mut hoch, false);
        assert!(hoch.d.iter().all(|v| (v - 0.4).abs() < 1e-6));
        // Rampe: Verdoppeln einer halbierten Rampe bleibt nahe der Rampe.
        let rampe = Ebene { d: (0..w * h).map(|i| (i % w) as f32).collect(), ax: 0, ay: 0, w, h };
        let g = halbieren(&rampe, 1, false);
        let mut z = Ebene { d: vec![0.0; w * h], ax: 0, ay: 0, w, h };
        verdoppeln_in(&g, 1, &mut z, false);
        let z = z.d;
        for y in 0..h {
            for x in 2..w - 2 {
                assert!((z[y * w + x] - x as f32).abs() < 0.3, "{x} {}", z[y * w + x]);
            }
        }
    }

    #[test]
    fn einzelbild_kommt_unveraendert_heraus() {
        // Ein Bild, mitten auf einer groesseren Leinwand: nach Pyramide und
        // Wiederaufbau muss jedes gedeckte Pixel exakt erhalten bleiben — auch an
        // den Bildkanten und am Leinwandrand (frueher: dunkler Saum).
        let (w, h) = (90usize, 70usize);
        let n_st = 4;
        let wert: Vec<f32> = (0..w * h * 3).map(|i| 0.2 + 0.5 * (((i * 2654435761usize) >> 7) % 97) as f32 / 97.0).collect();
        // Leinwand 0..120 x 0..80, Bild bei (25, 10)
        let (lw, lh) = (120usize, 80usize);
        let mut acc = Vec::new();
        let mut wacc = Vec::new();
        let (mut sw, mut sh) = (lw, lh);
        for k in 0..=n_st {
            if k > 0 {
                sw = sw.div_ceil(2);
                sh = sh.div_ceil(2);
            }
            let r = if k == 0 { 0 } else { 2 };
            acc.push(Ebene { d: vec![0.0; (sw + 2 * r) * (sh + 2 * r) * 3], ax: -(r as isize), ay: -(r as isize), w: sw + 2 * r, h: sh + 2 * r });
            wacc.push(Ebene { d: vec![0.0; (sw + 2 * r) * (sh + 2 * r)], ax: -(r as isize), ay: -(r as isize), w: sw + 2 * r, h: sh + 2 * r });
        }
        let mut gb = vec![Ebene { d: wert.clone(), ax: 25, ay: 10, w, h }];
        let mut gg = vec![Ebene { d: vec![1.0; w * h], ax: 25, ay: 10, w, h }];
        for k in 1..=n_st {
            let a = halbieren(&gb[k - 1], 3, false);
            let b = halbieren(&gg[k - 1], 1, true);
            gb.push(a);
            gg.push(b);
        }
        for k in 0..=n_st {
            let mut lap = std::mem::replace(&mut gb[k], Ebene { d: vec![], ax: 0, ay: 0, w: 0, h: 0 });
            if k < n_st {
                verdoppeln_in(&gb[k + 1], 3, &mut lap, true);
            }
            aufaddieren(&mut acc[k], &mut wacc[k], &lap, &gg[k]);
        }
        for k in 0..=n_st {
            let wk = std::mem::take(&mut wacc[k].d);
            acc[k].d.chunks_mut(3).zip(&wk).for_each(|(p, g)| {
                let i = if *g > 1e-12 { 1.0 / g } else { 0.0 };
                p.iter_mut().for_each(|v| *v *= i);
            });
        }
        for k in (0..n_st).rev() {
            let grob = std::mem::replace(&mut acc[k + 1], Ebene { d: vec![], ax: 0, ay: 0, w: 0, h: 0 });
            verdoppeln_in(&grob, 3, &mut acc[k], false);
        }
        let mut max = 0.0f32;
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    let soll = wert[(y * w + x) * 3 + c];
                    let ist = acc[0].d[((y + 10) * lw + x + 25) * 3 + c];
                    max = max.max((soll - ist).abs());
                }
            }
        }
        assert!(max < 1e-4, "groesste Abweichung {max}");
    }
}
