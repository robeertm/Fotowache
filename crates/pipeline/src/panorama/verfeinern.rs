//! Feinausrichtung auf voller Aufloesung.
//!
//! Die Merkmale stammen aus ~1500-px-Kopien. Bei einem 40-MP-Foto ist ein Pixel
//! dort fast fuenf echte Pixel breit — selbst ein sehr guter Ausgleich bleibt dann
//! bei ein bis zwei Pixeln Restfehler, und genau das sieht man an Kanten in der
//! Naht. Deshalb wird jedes Punktpaar auf voller Aufloesung nachgefuehrt: Feld um
//! den Punkt in Bild i als Vorlage, im Bild j an der vom Modell vorhergesagten
//! Stelle (samt oertlicher Verzerrung) gesucht, Verschiebung nach Lucas-Kanade
//! mit Helligkeits-Ausgleich (Faktor + Versatz, wegen unterschiedlicher Belichtung).
//!
//! Quellen: Lucas & Kanade 1981 (iterative Bildausrichtung); Szeliski,
//! "Image Alignment and Stitching: A Tutorial" (2006), Abschnitte 3.2/4.1
//! (Verfeinerung merkmalsbasierter Ausrichtung auf den Pixeln, affines
//! Feldmodell, Helligkeit mit Faktor und Versatz). Gemessen (13 Reihen,
//! Median des Nahtversatzes): mit Nachfuehrung in 7 Reihen kleiner, in 2
//! groesser (2094–2098: 2,34 statt 1,61 px — dort bewegte Personen), sonst
//! gleich.

use super::ausrichtung::{uebertragen, Modell, Optik, Paar};
use super::mathe::Mat3;
use super::merkmale::helligkeit;
use super::naht::abtasten;
use rayon::prelude::*;

pub struct VollBild<'a> {
    pub rgb: &'a [f32],
    pub w: usize,
    pub h: usize,
}

#[inline]
fn grau(b: &VollBild, x: f64, y: f64) -> f32 {
    let p = abtasten(b.rgb, b.w, b.h, x as f32, y as f32);
    helligkeit(p[0], p[1], p[2])
}

/// Fuehrt alle Punktpaare nach. `s`: Merkmalsmassstab (klein = voll * s),
/// `m`: Kameramodell in vollen Pixeln. Rueckgabe: Paare mit
/// Punkten in vollen Pixeln (zentriert); ein Paar, bei dem zu wenig Punkte
/// nachgefuehrt werden konnten, behaelt seine hochskalierten Grobpunkte.
pub fn nachfuehren(paare: &[Paar], bilder: &[VollBild], m: &Modell, s: f64) -> (Vec<Paar>, usize, usize) {
    let radius = ((5.0 / s).round() as isize).clamp(8, 24);
    let max_weg = (2.0f64).max(1.5 / s);
    let ergebnis: Vec<(Paar, usize, usize)> = paare
        .par_iter()
        .map(|p| {
            let (bi, bj) = (&bilder[p.i], &bilder[p.j]);
            let (ci, cj) = ((bi.w as f64 / 2.0, bi.h as f64 / 2.0), (bj.w as f64 / 2.0, bj.h as f64 / 2.0));
            let grob: Vec<[f64; 4]> = p.punkte.iter().map(|q| [q[0] / s, q[1] / s, q[2] / s, q[3] / s]).collect();
            let mut fein = Vec::with_capacity(grob.len());
            for q in &grob {
                if let Some(t) = ein_punkt(bi, bj, &m.r[p.i], &m.r[p.j], (m.optik(p.i), m.optik(p.j)), (m.diag[p.i], m.diag[p.j]), (q[0], q[1]), ci, cj, radius, max_weg) {
                    fein.push([q[0], q[1], t.0, t.1]);
                }
            }
            let (n, alle) = (fein.len(), grob.len());
            if n >= 8 && n * 3 >= alle {
                (Paar { punkte: fein, ..p.clone() }, n, alle)
            } else {
                (Paar { punkte: grob, ..p.clone() }, n, alle)
            }
        })
        .collect();
    let gut = ergebnis.iter().map(|e| e.1).sum();
    let alle = ergebnis.iter().map(|e| e.2).sum();
    (ergebnis.into_iter().map(|e| e.0).collect(), gut, alle)
}

#[allow(clippy::too_many_arguments)]
fn ein_punkt(
    bi: &VollBild,
    bj: &VollBild,
    ri: &Mat3,
    rj: &Mat3,
    o: (Optik, Optik),
    d: (f64, f64),
    pi: (f64, f64),
    ci: (f64, f64),
    cj: (f64, f64),
    radius: isize,
    max_weg: f64,
) -> Option<(f64, f64)> {
    let vor = |x: f64, y: f64| uebertragen(ri, rj, o.0, o.1, d.0, d.1, x, y);
    let pj = vor(pi.0, pi.1)?;
    // Oertliche Verzerrung i -> j (Jacobi-Matrix) aus Differenzen.
    let (ax, ay) = (vor(pi.0 + 1.0, pi.1)?, vor(pi.0, pi.1 + 1.0)?);
    let a = [[ax.0 - pj.0, ay.0 - pj.0], [ax.1 - pj.1, ay.1 - pj.1]];
    let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
    if det.abs() < 1e-6 {
        return None;
    }
    // A^-T fuer die Gradienten-Umrechnung
    let ait = [[a[1][1] / det, -a[1][0] / det], [-a[0][1] / det, a[0][0] / det]];
    let n = (2 * radius + 1) as usize;
    let rand = radius as f64 * 1.5 + max_weg + 2.0;
    let (gx, gy) = (pi.0 + ci.0, pi.1 + ci.1);
    if gx < rand || gy < rand || gx > bi.w as f64 - rand || gy > bi.h as f64 - rand {
        return None;
    }
    let (hx, hy) = (pj.0 + cj.0, pj.1 + cj.1);
    if hx < rand || hy < rand || hx > bj.w as f64 - rand || hy > bj.h as f64 - rand {
        return None;
    }
    // Vorlage mit einem Pixel Rand fuer die Gradienten.
    let m = n + 2;
    let mut t = vec![0.0f32; m * m];
    for y in 0..m {
        for x in 0..m {
            let o = (x as f64 - radius as f64 - 1.0, y as f64 - radius as f64 - 1.0);
            t[y * m + x] = grau(bi, gx + o.0, gy + o.1);
        }
    }
    let mut tv = Vec::with_capacity(n * n);
    let mut grad = Vec::with_capacity(n * n);
    let (mut mittel, mut quad) = (0.0f64, 0.0f64);
    for y in 1..=n {
        for x in 1..=n {
            let v = t[y * m + x] as f64;
            let dx = 0.5 * (t[y * m + x + 1] - t[y * m + x - 1]) as f64;
            let dy = 0.5 * (t[(y + 1) * m + x] - t[(y - 1) * m + x]) as f64;
            // Gradient der Vorlage in Koordinaten von Bild j: A^-T * grad
            grad.push([ait[0][0] * dx + ait[0][1] * dy, ait[1][0] * dx + ait[1][1] * dy]);
            tv.push(v);
            mittel += v;
            quad += v * v;
        }
    }
    let nn = (n * n) as f64;
    let var_t = quad / nn - (mittel / nn).powi(2);
    if var_t < 1e-7 {
        return None; // strukturlos — nicht nachfuehrbar
    }
    let (mut tx, mut ty) = (0.0f64, 0.0f64);
    let (mut ga, mut gb) = (1.0f64, 0.0f64);
    let mut jv = vec![0.0f64; n * n];
    let mut konvergiert = false;
    for _ in 0..20 {
        let mut k = 0;
        for y in 0..n {
            for x in 0..n {
                let o = (x as f64 - radius as f64, y as f64 - radius as f64);
                let qx = hx + a[0][0] * o.0 + a[0][1] * o.1 + tx;
                let qy = hy + a[1][0] * o.0 + a[1][1] * o.1 + ty;
                jv[k] = grau(bj, qx, qy) as f64;
                k += 1;
            }
        }
        // Modell: T ~ ga * J(q + t) + gb. Unbekannte: dt (2), dga, dgb.
        // Ableitung nach t: ga * grad J ~ grad T (in j-Koordinaten).
        let mut h = [0.0f64; 16];
        let mut g = [0.0f64; 4];
        for k in 0..n * n {
            let e = ga * jv[k] + gb - tv[k];
            let z = [grad[k][0], grad[k][1], jv[k], 1.0];
            for u in 0..4 {
                g[u] += z[u] * e;
                for v in 0..4 {
                    h[u * 4 + v] += z[u] * z[v];
                }
            }
        }
        let mut rechts = [-g[0], -g[1], -g[2], -g[3]];
        let d = super::mathe::loesen(&mut h, &mut rechts, 4)?;
        tx += d[0];
        ty += d[1];
        ga += d[2];
        gb += d[3];
        if !(ga.is_finite() && ga > 0.1 && ga < 10.0) || tx.abs() > max_weg || ty.abs() > max_weg {
            return None;
        }
        if d[0].abs() < 0.003 && d[1].abs() < 0.003 {
            konvergiert = true;
            break;
        }
    }
    if !konvergiert {
        return None;
    }
    // Endkontrolle: normierte Kreuzkorrelation der Felder.
    let mj = jv.iter().sum::<f64>() / nn;
    let mt = mittel / nn;
    let (mut sjt, mut sjj, mut stt) = (0.0, 0.0, 0.0);
    for k in 0..n * n {
        let (u, v) = (jv[k] - mj, tv[k] - mt);
        sjt += u * v;
        sjj += u * u;
        stt += v * v;
    }
    let ncc = sjt / (sjj * stt).sqrt().max(1e-300);
    if ncc < 0.85 {
        return None;
    }
    Some((pj.0 + tx, pj.1 + ty))
}
