//! Paarweise Geometrie: Homographie per RANSAC und Brennweite aus Homographien.
//!
//! Zwei Aufnahmen vom selben Standpunkt (nur gedreht) haengen exakt ueber eine
//! Homographie H = K R K^-1 zusammen. Deshalb wird pro Paar zuerst die allgemeine
//! Homographie robust geschaetzt (RANSAC verwirft Fehlzuordnungen), und erst
//! danach das strengere Drehmodell fuer alle Bilder gemeinsam angepasst.
//!
//! RANSAC nach Brown & Lowe (IJCV 2007, Abschnitt 3.1): Stichproben aus r = 4
//! Punktpaaren, Homographie per DLT, n = 500 Versuche, gewaehlt wird die mit den
//! meisten Innenpunkten. Die Toleranz ε nennt die Arbeit nicht; genommen ist
//! der Standardwert der Referenzumsetzung von Brown & Lowe (3 px). Entartete Stichproben werden wie dort verworfen (drei Punkte auf einer
//! Geraden; Umlaufsinn der Dreiecke nicht in allen vier Teildreiecken gleich —
//! Marquez-Neila u. a. 2013, "Speeding-up homography estimation in mobile
//! devices"). Danach wird auf allen Innenpunkten neu geschaetzt (DLT nach
//! Hartley & Zisserman, Alg. 4.2, normiert).

use super::mathe::{loesen, eigen_sym, Mat3, Zufall};

pub type P2 = (f64, f64);

#[inline]
pub fn abbilden(h: &Mat3, p: P2) -> P2 {
    let x = h[0][0] * p.0 + h[0][1] * p.1 + h[0][2];
    let y = h[1][0] * p.0 + h[1][1] * p.1 + h[1][2];
    let z = h[2][0] * p.0 + h[2][1] * p.1 + h[2][2];
    if z.abs() < 1e-12 {
        return (f64::INFINITY, f64::INFINITY);
    }
    (x / z, y / z)
}

/// Normierung nach Hartley (Schwerpunkt 0, mittlerer Abstand sqrt 2). Ohne sie
/// ist das DLT-System bei Pixelkoordinaten um 1000 voellig schlecht konditioniert.
fn normierung(p: &[P2]) -> Mat3 {
    let n = p.len().max(1) as f64;
    let (mx, my) = p.iter().fold((0.0, 0.0), |a, q| (a.0 + q.0, a.1 + q.1));
    let (mx, my) = (mx / n, my / n);
    let d = p.iter().map(|q| ((q.0 - mx).powi(2) + (q.1 - my).powi(2)).sqrt()).sum::<f64>() / n;
    let s = if d > 1e-12 { std::f64::consts::SQRT_2 / d } else { 1.0 };
    [[s, 0.0, -s * mx], [0.0, s, -s * my], [0.0, 0.0, 1.0]]
}

fn inv_normierung(t: &Mat3) -> Mat3 {
    let s = t[0][0];
    [[1.0 / s, 0.0, -t[0][2] / s], [0.0, 1.0 / s, -t[1][2] / s], [0.0, 0.0, 1.0]]
}

/// Exakte Homographie aus 4 Punktpaaren (h33 = 1, 8x8-System).
fn vier_punkte(p: &[P2; 4], q: &[P2; 4]) -> Option<Mat3> {
    let mut a = [0.0f64; 64];
    let mut b = [0.0f64; 8];
    for k in 0..4 {
        let (x, y) = p[k];
        let (u, v) = q[k];
        let r1 = 2 * k;
        let r2 = 2 * k + 1;
        a[r1 * 8..r1 * 8 + 8].copy_from_slice(&[x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y]);
        b[r1] = u;
        a[r2 * 8..r2 * 8 + 8].copy_from_slice(&[0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y]);
        b[r2] = v;
    }
    let h = loesen(&mut a, &mut b, 8)?;
    Some([[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], 1.0]])
}

/// Kleinste-Quadrate-Homographie (DLT) aus vielen Paaren, in normierten
/// Koordinaten; Loesung = Eigenvektor zum kleinsten Eigenwert von A^T A.
pub fn dlt(p: &[P2], q: &[P2]) -> Option<Mat3> {
    if p.len() < 4 {
        return None;
    }
    let tp = normierung(p);
    let tq = normierung(q);
    let mut ata = [0.0f64; 81];
    for (a, b) in p.iter().zip(q) {
        let (x, y) = (tp[0][0] * a.0 + tp[0][2], tp[1][1] * a.1 + tp[1][2]);
        let (u, v) = (tq[0][0] * b.0 + tq[0][2], tq[1][1] * b.1 + tq[1][2]);
        let z1 = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, -u];
        let z2 = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, -v];
        for i in 0..9 {
            for j in 0..9 {
                ata[i * 9 + j] += z1[i] * z1[j] + z2[i] * z2[j];
            }
        }
    }
    let (_, v) = eigen_sym(&ata, 9);
    let h: Vec<f64> = (0..9).map(|k| v[k * 9]).collect();
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]];
    // Entnormieren: H = Tq^-1 Hn Tp
    let hm = super::mathe::mul(&super::mathe::mul(&inv_normierung(&tq), &hn), &tp);
    if hm[2][2].abs() < 1e-12 {
        return None;
    }
    Some(super::mathe::skalieren(&hm, 1.0 / hm[2][2]))
}

fn endlich(h: &Mat3) -> bool {
    h.iter().flatten().all(|v| v.is_finite())
}

/// Drei der vier Punkte (fast) auf einer Geraden?
fn kollinear(p: &[P2; 4]) -> bool {
    for (a, b, c) in [(0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3)] {
        let (dx1, dy1) = (p[b].0 - p[a].0, p[b].1 - p[a].1);
        let (dx2, dy2) = (p[c].0 - p[a].0, p[c].1 - p[a].1);
        // wie die Referenzumsetzung: |Kreuzprodukt| <= eps * (|d1| * |d2|)
        if (dx2 * dy1 - dy2 * dx1).abs() <= f32::EPSILON as f64 * (dx1.abs() + dy1.abs() + dx2.abs() + dy2.abs()) {
            return true;
        }
    }
    false
}

/// Stichprobe geometrisch stimmig (Umlaufsinn aller vier Teildreiecke in beiden
/// Bildern gleich oder in allen gespiegelt)?
fn stimmig(p: &[P2; 4], q: &[P2; 4]) -> bool {
    let det = |a: P2, b: P2, c: P2| (b.0 - a.0) * (c.1 - a.1) - (c.0 - a.0) * (b.1 - a.1);
    let mut negativ = 0;
    for t in [[0, 1, 2], [1, 2, 3], [0, 2, 3], [0, 1, 3]] {
        if det(p[t[0]], p[t[1]], p[t[2]]) * det(q[t[0]], q[t[1]], q[t[2]]) < 0.0 {
            negativ += 1;
        }
    }
    negativ == 0 || negativ == 4
}

pub struct RansacErgebnis {
    pub h: Mat3,
    pub innen: Vec<usize>,
}

/// Versuche je RANSAC (B&L Abschnitt 3.1: n = 500).
pub const RANSAC_VERSUCHE: usize = 500;
/// Toleranz in Pixeln der Punktkoordinaten (Standardwert der Referenzumsetzung).
pub const RANSAC_TOLERANZ: f64 = 3.0;

/// Robuste Homographie p -> q. `schwelle` = Rueckprojektionsfehler in Pixeln.
pub fn ransac(p: &[P2], q: &[P2], schwelle: f64, saat: u64) -> Option<RansacErgebnis> {
    let n = p.len();
    if n < 4 {
        return None;
    }
    let s2 = schwelle * schwelle;
    let zaehlen = |h: &Mat3| -> usize {
        p.iter().zip(q).filter(|(a, b)| {
            let r = abbilden(h, **a);
            (r.0 - b.0).powi(2) + (r.1 - b.1).powi(2) < s2
        }).count()
    };
    let mut z = Zufall::neu(saat);
    let mut beste: Option<(usize, Mat3)> = None;
    for _ in 0..RANSAC_VERSUCHE {
        let mut idx = [0usize; 4];
        let mut k = 0;
        let mut versuche = 0;
        while k < 4 && versuche < 50 {
            versuche += 1;
            let c = z.bis(n);
            if !idx[..k].contains(&c) {
                idx[k] = c;
                k += 1;
            }
        }
        if k < 4 {
            continue;
        }
        let pp = [p[idx[0]], p[idx[1]], p[idx[2]], p[idx[3]]];
        let qq = [q[idx[0]], q[idx[1]], q[idx[2]], q[idx[3]]];
        if kollinear(&pp) || kollinear(&qq) || !stimmig(&pp, &qq) {
            continue;
        }
        let Some(h) = vier_punkte(&pp, &qq) else { continue };
        if !endlich(&h) {
            continue;
        }
        let c = zaehlen(&h);
        if beste.as_ref().is_none_or(|b| c > b.0) {
            beste = Some((c, h));
        }
    }
    let (_, mut h) = beste?;
    // Nachfassen: auf allen Innenpunkten neu ausgleichen, Innenpunkte neu zaehlen.
    let mut innen: Vec<usize> = Vec::new();
    for _ in 0..4 {
        let neu: Vec<usize> = (0..n)
            .filter(|&k| {
                let r = abbilden(&h, p[k]);
                (r.0 - q[k].0).powi(2) + (r.1 - q[k].1).powi(2) < s2
            })
            .collect();
        if neu.len() < 4 {
            return None;
        }
        let gleich = neu == innen;
        innen = neu;
        if gleich {
            break;
        }
        let pi: Vec<P2> = innen.iter().map(|&k| p[k]).collect();
        let qi: Vec<P2> = innen.iter().map(|&k| q[k]).collect();
        match dlt(&pi, &qi) {
            Some(neu_h) if endlich(&neu_h) => h = neu_h,
            _ => break,
        }
    }
    Some(RansacErgebnis { h, innen })
}

/// Brennweiten-Kandidat aus einer Homographie zwischen zwei Bildern mit
/// Ursprung in der Bildmitte (Ansatz nach Szeliski/Shum: K^-1 H K muss eine
/// Drehung sein -> Bedingungen an Spalten- bzw. Zeilenorthogonalitaet).
/// Liefert sqrt(f0 * f1) oder `None`, wenn das Paar die Brennweite nicht
/// festlegt (z. B. reine Verschiebung — dann ist jede grosse Brennweite gleich gut).
/// Wie die Brennweitenschaetzung der Referenzumsetzung von Brown & Lowe (beim
/// Tauschen der Kandidaten werden die Nenner mitgetauscht).
pub fn brennweite_aus_h(h: &Mat3) -> Option<f64> {
    let h = [h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1], h[2][2]];
    let f1 = {
        let mut d1 = h[6] * h[7];
        let mut d2 = (h[7] - h[6]) * (h[7] + h[6]);
        let mut v1 = -(h[0] * h[1] + h[3] * h[4]) / d1;
        let mut v2 = (h[0] * h[0] + h[3] * h[3] - h[1] * h[1] - h[4] * h[4]) / d2;
        if v1 < v2 {
            std::mem::swap(&mut v1, &mut v2);
            std::mem::swap(&mut d1, &mut d2);
        }
        if v1 > 0.0 && v2 > 0.0 {
            Some((if d1.abs() > d2.abs() { v1 } else { v2 }).sqrt())
        } else if v1 > 0.0 {
            Some(v1.sqrt())
        } else {
            None
        }
    };
    let f0 = {
        let mut d1 = h[0] * h[3] + h[1] * h[4];
        let mut d2 = h[0] * h[0] + h[1] * h[1] - h[3] * h[3] - h[4] * h[4];
        let mut v1 = -h[2] * h[5] / d1;
        let mut v2 = (h[5] * h[5] - h[2] * h[2]) / d2;
        if v1 < v2 {
            std::mem::swap(&mut v1, &mut v2);
            std::mem::swap(&mut d1, &mut d2);
        }
        if v1 > 0.0 && v2 > 0.0 {
            Some((if d1.abs() > d2.abs() { v1 } else { v2 }).sqrt())
        } else if v1 > 0.0 {
            Some(v1.sqrt())
        } else {
            None
        }
    };
    match (f0, f1) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => Some((a * b).sqrt()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panorama::mathe::{drehung, mul, transp};

    #[test]
    fn brennweite_aus_drehung() {
        let f = 900.0;
        let k = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let ki = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
        let r = drehung(&[0.05, 0.4, 0.02]);
        let h = mul(&mul(&k, &transp(&r)), &ki);
        let g = brennweite_aus_h(&h).unwrap();
        assert!((g - f).abs() < 1.0, "{g}");
    }

    #[test]
    fn ransac_findet_h() {
        let h = [[1.02, 0.03, 40.0], [-0.02, 0.99, -12.0], [1e-5, -2e-5, 1.0]];
        let mut z = Zufall::neu(7);
        let mut p = vec![];
        let mut q = vec![];
        for k in 0..200 {
            let a = (z.gleich() * 800.0, z.gleich() * 600.0);
            let b = if k % 3 == 0 { (z.gleich() * 800.0, z.gleich() * 600.0) } else { abbilden(&h, a) };
            p.push(a);
            q.push(b);
        }
        let e = ransac(&p, &q, 1.0, 1).unwrap();
        assert!(e.innen.len() >= 130);
        let r = abbilden(&e.h, (400.0, 300.0));
        let s = abbilden(&h, (400.0, 300.0));
        assert!((r.0 - s.0).abs() < 1e-3 && (r.1 - s.1).abs() < 1e-3);
    }
}
