//! Direkte Ausrichtung zweier Bilder: Homographie ueber die Pixel der
//! Ueberlappung, grob bis fein.
//!
//! ## Warum
//! Gemessen an einer echten Reihe (vom fahrenden Schiff aus): die Fassade eines
//! langen Speichers erscheint im einen Bild waagrecht um ein Drittel gestaucht,
//! im anderen nicht; dazu ist sie ein einziges Fensterraster. Punktweise
//! Vergleiche finden dort fast nur falsche Nachbarfenster. Eine Hypothese aus den
//! wenigen richtigen Punkten liegt 20-50 px daneben — zu weit fuer eine Suche im
//! kleinen Fenster, und ein grosses Fenster ist im Raster mehrdeutig.
//!
//! Die direkte Ausrichtung passt stattdessen alle acht Homographie-Parameter
//! (plus Helligkeit: Faktor und Versatz) gleichzeitig an alle strukturierten
//! Pixel an — auf der groebsten Stufe (1/8) ist das Raster weggeglaettet, dort
//! zaehlen Dachkante, Tuerme und Umrisse, und der Fangbereich ist ein paar
//! Dutzend Pixel. Danach Stufe fuer Stufe feiner. Huber-Gewichte halten bewegtes
//! Wasser und Parallaxe klein.

use super::geometrie::abbilden;
use super::mathe::{loesen, Mat3};
use super::merkmale::Grau;

struct Stufe {
    g: Grau,
    gx: Vec<f32>,
    gy: Vec<f32>,
}

pub(super) fn halbieren(g: &Grau) -> Grau {
    let (w, h) = (g.w / 2, g.h / 2);
    let mut d = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = 2 * y * g.w + 2 * x;
            d[y * w + x] = 0.25 * (g.d[i] + g.d[i + 1] + g.d[i + g.w] + g.d[i + g.w + 1]);
        }
    }
    Grau { w, h, d }
}

fn stufe(g: Grau) -> Stufe {
    let (w, h) = (g.w, g.h);
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            gx[y * w + x] = 0.5 * (g.d[y * w + x + 1] - g.d[y * w + x - 1]);
            gy[y * w + x] = 0.5 * (g.d[(y + 1) * w + x] - g.d[(y - 1) * w + x]);
        }
    }
    Stufe { g, gx, gy }
}

/// Pyramide bis die laengere Seite hoechstens `bis` Pixel hat.
fn pyramide(g: &Grau, bis: usize) -> Vec<Stufe> {
    let mut v = vec![Grau { w: g.w, h: g.h, d: g.d.clone() }];
    while v.last().unwrap().w.max(v.last().unwrap().h) > bis && v.last().unwrap().w >= 32 && v.last().unwrap().h >= 32 {
        let n = halbieren(v.last().unwrap());
        v.push(n);
    }
    v.into_iter().map(stufe).collect()
}

#[inline]
fn bilinear(d: &[f32], w: usize, h: usize, x: f64, y: f64) -> f32 {
    let fx = (x - 0.5).clamp(0.0, (w - 1) as f64);
    let fy = (y - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (ax, ay) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
    let a = d[y0 * w + x0] * (1.0 - ax) + d[y0 * w + x1] * ax;
    let b = d[y1 * w + x0] * (1.0 - ax) + d[y1 * w + x1] * ax;
    a + (b - a) * ay
}

/// Ergebnis der direkten Ausrichtung.
pub struct Direkt {
    /// Homographie a -> b in zentrierten Koordinaten der Ausgangsstufe.
    pub h: Mat3,
    /// Normierte Kreuzkorrelation der Ueberlappung auf der feinsten Stufe.
    pub korrelation: f64,
    /// Anteil der Pixel von `a`, die in `b` liegen.
    pub anteil: f64,
}

/// Richtet `a` auf `b` aus, ausgehend von `h0` (a -> b, zentriert; Mitten
/// `ma`/`mb` in Pixeln der Ausgangsstufe). `None`, wenn die Ueberlappung zu
/// klein wird oder die Rechnung entgleist.
pub fn ausrichten(a: &Grau, ma: (f64, f64), b: &Grau, mb: (f64, f64), h0: &Mat3) -> Option<Direkt> {
    let pa = pyramide(a, 200);
    let pb = pyramide(b, 200);
    let stufen = pa.len().min(pb.len());
    // Normierte Koordinaten (Teilung durch c) halten das System gut konditioniert
    // und sind auf allen Stufen dieselben.
    let c = (a.w.max(a.h) as f64) / 2.0;
    let mut hn = [
        [h0[0][0], h0[0][1], h0[0][2] / c],
        [h0[1][0], h0[1][1], h0[1][2] / c],
        [h0[2][0] * c, h0[2][1] * c, h0[2][2]],
    ];
    let norm = hn[2][2];
    if norm.abs() < 1e-12 {
        return None;
    }
    for z in hn.iter_mut() {
        for v in z.iter_mut() {
            *v /= norm;
        }
    }
    let (mut gain, mut bias) = (1.0f64, 0.0f64);
    let mut ergebnis_korr = 0.0;
    let mut anteil = 0.0;
    for l in (0..stufen).rev() {
        let (sa, sb) = (&pa[l], &pb[l]);
        let sig = 0.5f64.powi(l as i32);
        let cl = c * sig;
        let (mal, mbl) = ((ma.0 * sig, ma.1 * sig), (mb.0 * sig, mb.1 * sig));
        // Stichproben: strukturierte Pixel von a, hoechstens ~30000.
        let schritt = (((sa.g.w * sa.g.h) as f64 / 30000.0).sqrt().ceil() as usize).max(1);
        let mut proben: Vec<(f64, f64, f32)> = Vec::new();
        let mut alle = 0usize;
        for y in (2..sa.g.h.saturating_sub(2)).step_by(schritt) {
            for x in (2..sa.g.w.saturating_sub(2)).step_by(schritt) {
                alle += 1;
                let i = y * sa.g.w + x;
                if sa.gx[i].abs() + sa.gy[i].abs() < 0.004 {
                    continue;
                }
                proben.push(((x as f64 + 0.5 - mal.0) / cl, (y as f64 + 0.5 - mal.1) / cl, sa.g.d[i]));
            }
        }
        let iterationen = if l == 0 { 8 } else { 15 };
        let mut lambda = 1e-3;
        // Kosten und Normalgleichungen fuer gegebene Parameter.
        let auswerten = |hn: &Mat3, gain: f64, bias: f64, mit_jac: bool| -> (f64, usize, [f64; 100], [f64; 10]) {
            let mut jtj = [0.0f64; 100];
            let mut jtr = [0.0f64; 10];
            let mut kosten = 0.0;
            let mut n = 0usize;
            const K: f64 = 0.03;
            for &(xn, yn, av) in &proben {
                let w = hn[2][0] * xn + hn[2][1] * yn + 1.0;
                if w <= 1e-6 {
                    continue;
                }
                let un = (hn[0][0] * xn + hn[0][1] * yn + hn[0][2]) / w;
                let vn = (hn[1][0] * xn + hn[1][1] * yn + hn[1][2]) / w;
                let (u, v) = (un * cl + mbl.0, vn * cl + mbl.1);
                if u < 2.0 || v < 2.0 || u > sb.g.w as f64 - 2.0 || v > sb.g.h as f64 - 2.0 {
                    continue;
                }
                n += 1;
                let bv = bilinear(&sb.g.d, sb.g.w, sb.g.h, u, v) as f64;
                let r = gain * bv + bias - av as f64;
                let e = r.abs();
                let (k, gw) = if e <= K { (0.5 * r * r, 1.0) } else { (K * (e - 0.5 * K), K / e) };
                kosten += k;
                if !mit_jac {
                    continue;
                }
                // Ableitung nach u_n, v_n: Gradient (Pixel) * cl
                let gx = bilinear(&sb.gx, sb.g.w, sb.g.h, u, v) as f64 * cl * gain;
                let gy = bilinear(&sb.gy, sb.g.w, sb.g.h, u, v) as f64 * cl * gain;
                let j = [
                    gx * xn / w,
                    gx * yn / w,
                    gx / w,
                    gy * xn / w,
                    gy * yn / w,
                    gy / w,
                    -(gx * un + gy * vn) * xn / w,
                    -(gx * un + gy * vn) * yn / w,
                    bv,
                    1.0,
                ];
                for p in 0..10 {
                    jtr[p] += gw * j[p] * r;
                    for q in 0..10 {
                        jtj[p * 10 + q] += gw * j[p] * j[q];
                    }
                }
            }
            (kosten, n, jtj, jtr)
        };
        for _ in 0..iterationen {
            let (k0, n, jtj, jtr) = auswerten(&hn, gain, bias, true);
            if n < 200 {
                return None;
            }
            let fertig;
            loop {
                let mut a = jtj;
                for d in 0..10 {
                    a[d * 10 + d] += lambda * jtj[d * 10 + d].max(1e-12);
                }
                let mut rechts: Vec<f64> = jtr.iter().map(|v| -v).collect();
                let d = loesen(&mut a, &mut rechts, 10)?;
                let neu = [[hn[0][0] + d[0], hn[0][1] + d[1], hn[0][2] + d[2]], [hn[1][0] + d[3], hn[1][1] + d[4], hn[1][2] + d[5]], [hn[2][0] + d[6], hn[2][1] + d[7], 1.0]];
                let (gn, bn) = (gain + d[8], bias + d[9]);
                let (k1, n1, _, _) = auswerten(&neu, gn, bn, false);
                // Kosten pro Stichprobe vergleichen (die Ueberlappung darf sich aendern).
                if n1 >= 200 && k1 / n1 as f64 <= k0 / n as f64 {
                    let schritt = d[..8].iter().map(|v| v.abs()).fold(0.0, f64::max);
                    hn = neu;
                    gain = gn;
                    bias = bn;
                    lambda = (lambda / 3.0).max(1e-8);
                    fertig = schritt < 1e-5;
                    break;
                }
                lambda *= 5.0;
                if lambda > 1e6 {
                    fertig = true;
                    break;
                }
            }
            if fertig {
                break;
            }
        }
        if !(0.2..5.0).contains(&gain) {
            return None;
        }
        if l == 0 {
            // Abschlusspruefung: Korrelation der ueberlappenden Stichproben.
            let (mut sa_, mut sb_, mut saa, mut sbb, mut sab, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            for &(xn, yn, av) in &proben {
                let w = hn[2][0] * xn + hn[2][1] * yn + 1.0;
                if w <= 1e-6 {
                    continue;
                }
                let u = (hn[0][0] * xn + hn[0][1] * yn + hn[0][2]) / w * cl + mbl.0;
                let v = (hn[1][0] * xn + hn[1][1] * yn + hn[1][2]) / w * cl + mbl.1;
                if u < 2.0 || v < 2.0 || u > sb.g.w as f64 - 2.0 || v > sb.g.h as f64 - 2.0 {
                    continue;
                }
                let bv = bilinear(&sb.g.d, sb.g.w, sb.g.h, u, v) as f64;
                let av = av as f64;
                sa_ += av;
                sb_ += bv;
                saa += av * av;
                sbb += bv * bv;
                sab += av * bv;
                n += 1.0;
            }
            if n < 200.0 {
                return None;
            }
            let cov = sab / n - sa_ / n * sb_ / n;
            let va = saa / n - (sa_ / n).powi(2);
            let vb = sbb / n - (sb_ / n).powi(2);
            ergebnis_korr = cov / (va * vb).sqrt().max(1e-12);
            anteil = n / alle.max(1) as f64;
        }
    }
    let h = [
        [hn[0][0], hn[0][1], hn[0][2] * c],
        [hn[1][0], hn[1][1], hn[1][2] * c],
        [hn[2][0] / c, hn[2][1] / c, 1.0],
    ];
    // Plausibel? (keine Spiegelung, kein Massstabssprung ueber Faktor 3)
    let d = h[0][0] * h[1][1] - h[0][1] * h[1][0];
    if !(1.0 / 9.0..9.0).contains(&d) || !h.iter().flatten().all(|v| v.is_finite()) {
        return None;
    }
    let _ = abbilden;
    Some(Direkt { h, korrelation: ergebnis_korr, anteil })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panorama::merkmale::grau_aus_rgb;

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn findet_homographie_aus_grober_schaetzung() {
        // Szene, daraus b = a unter einer Homographie mit waagrechter Stauchung;
        // Start 25 px und 8 % daneben.
        let (w, h) = (600usize, 400usize);
        let sz = crate::panorama::tests::szene(w, h, 3);
        let a = grau_aus_rgb(&sz, w, h);
        let wahr = [[1.25, 0.02, -140.0], [0.01, 1.0, 30.0], [0.0002, 0.0, 1.0]];
        let m = (w as f64 / 2.0, h as f64 / 2.0);
        // b(q) = a(H^-1 q): fuer jedes b-Pixel den Urbildpunkt per Iteration suchen
        let inv = {
            let hh = wahr;
            let det = super::super::mathe::det(&hh);
            let mut r = [[0.0; 3]; 3];
            for i in 0..3 {
                for j in 0..3 {
                    let (a1, b1) = ((j + 1) % 3, (j + 2) % 3);
                    let (c1, e1) = ((i + 1) % 3, (i + 2) % 3);
                    r[i][j] = (hh[a1][c1] * hh[b1][e1] - hh[a1][e1] * hh[b1][c1]) / det;
                }
            }
            r
        };
        let mut bd = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let p = abbilden(&inv, (x as f64 + 0.5 - m.0, y as f64 + 0.5 - m.1));
                bd[y * w + x] = bilinear(&a.d, w, h, p.0 + m.0, p.1 + m.1);
            }
        }
        let b = Grau { w, h, d: bd };
        let start = [[1.15, 0.0, -115.0], [0.0, 1.0, 15.0], [0.0, 0.0, 1.0]];
        let e = ausrichten(&a, m, &b, m, &start).expect("Ausrichtung");
        for p in [(0.0, 0.0), (150.0, 80.0), (-100.0, -60.0)] {
            let s = abbilden(&wahr, p);
            let i = abbilden(&e.h, p);
            assert!((s.0 - i.0).abs() < 0.5 && (s.1 - i.1).abs() < 0.5, "{p:?}: soll {s:?}, ist {i:?}");
        }
        assert!(e.korrelation > 0.95, "{}", e.korrelation);
    }
}
