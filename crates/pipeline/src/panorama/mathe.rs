//! Kleine lineare Algebra fuer die Ausrichtung.
//!
//! Bewusst selbst geschrieben statt einer grossen Matrixbibliothek: gebraucht
//! werden nur 3x3-Drehungen, ein symmetrisches Eigenwertproblem (Jacobi) und das
//! Loesen kleiner dichter Gleichungssysteme (Homographie, Buendelausgleich mit
//! hoechstens ein paar hundert Unbekannten). Das sind ein paar Dutzend Zeilen —
//! eine Abhaengigkeit dafuer kostet mehr Bauzeit als sie spart.

pub type Mat3 = [[f64; 3]; 3];
pub type Vek3 = [f64; 3];

pub const EINHEIT: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut c = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            c[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    c
}

pub fn transp(a: &Mat3) -> Mat3 {
    let mut c = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            c[i][j] = a[j][i];
        }
    }
    c
}

pub fn mal_vek(a: &Mat3, v: &Vek3) -> Vek3 {
    [
        a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
        a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
        a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
    ]
}

pub fn det(a: &Mat3) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

pub fn skalieren(a: &Mat3, s: f64) -> Mat3 {
    let mut c = *a;
    for z in c.iter_mut() {
        for v in z.iter_mut() {
            *v *= s;
        }
    }
    c
}

pub fn kreuz(a: &Vek3, b: &Vek3) -> Vek3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn skalar(a: &Vek3, b: &Vek3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn normiert(a: &Vek3) -> Vek3 {
    let n = skalar(a, a).sqrt().max(1e-300);
    [a[0] / n, a[1] / n, a[2] / n]
}

/// Drehmatrix aus einem Drehvektor (Achse * Winkel), Rodrigues-Formel.
pub fn drehung(v: &Vek3) -> Mat3 {
    let t = skalar(v, v).sqrt();
    if t < 1e-12 {
        // Erste Ordnung reicht hier und vermeidet die Division durch null.
        return [[1.0, -v[2], v[1]], [v[2], 1.0, -v[0]], [-v[1], v[0], 1.0]];
    }
    let (x, y, z) = (v[0] / t, v[1] / t, v[2] / t);
    let (s, c) = t.sin_cos();
    let k = 1.0 - c;
    [
        [c + x * x * k, x * y * k - z * s, x * z * k + y * s],
        [y * x * k + z * s, c + y * y * k, y * z * k - x * s],
        [z * x * k - y * s, z * y * k + x * s, c + z * z * k],
    ]
}

/// Eigenzerlegung einer symmetrischen n x n-Matrix (zeilenweise) nach Jacobi.
/// Liefert (Eigenwerte, Eigenvektoren als Spalten von `v`), aufsteigend sortiert.
/// Jacobi ist langsam fuer grosse n, aber fuer n <= 9 unschlagbar robust.
pub fn eigen_sym(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut a = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..100 {
        let mut aus = 0.0;
        for p in 0..n {
            for q in p + 1..n {
                aus += a[p * n + q] * a[p * n + q];
            }
        }
        if aus < 1e-30 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = a[k * n + p];
                    let akq = a[k * n + q];
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[p * n + k];
                    let aqk = a[q * n + k];
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&x, &y| a[x * n + x].partial_cmp(&a[y * n + y]).unwrap_or(std::cmp::Ordering::Equal));
    let werte: Vec<f64> = idx.iter().map(|&i| a[i * n + i]).collect();
    let mut vek = vec![0.0; n * n];
    for (neu, &alt) in idx.iter().enumerate() {
        for k in 0..n {
            vek[k * n + neu] = v[k * n + alt];
        }
    }
    (werte, vek)
}

/// Loest A x = b (A dicht, n x n, zeilenweise) per Gauss mit Spaltenpivot.
/// `None`, wenn A (numerisch) singulaer ist.
pub fn loesen(a: &mut [f64], b: &mut [f64], n: usize) -> Option<Vec<f64>> {
    for k in 0..n {
        let mut p = k;
        let mut max = a[k * n + k].abs();
        for i in k + 1..n {
            let v = a[i * n + k].abs();
            if v > max {
                max = v;
                p = i;
            }
        }
        if max < 1e-14 {
            return None;
        }
        if p != k {
            for j in 0..n {
                a.swap(k * n + j, p * n + j);
            }
            b.swap(k, p);
        }
        let piv = a[k * n + k];
        for i in k + 1..n {
            let f = a[i * n + k] / piv;
            if f == 0.0 {
                continue;
            }
            for j in k..n {
                a[i * n + j] -= f * a[k * n + j];
            }
            b[i] -= f * b[k];
        }
    }
    let mut x = vec![0.0; n];
    for k in (0..n).rev() {
        let mut s = b[k];
        for j in k + 1..n {
            s -= a[k * n + j] * x[j];
        }
        x[k] = s / a[k * n + k];
    }
    Some(x)
}

/// Cholesky-Loesung fuer symmetrisch positiv definite Systeme (Normalgleichungen
/// des Buendelausgleichs). Halb so teuer wie Gauss und meldet zuverlaessig,
/// wenn die Daempfung nicht reicht (dann `None`).
pub fn cholesky_loesen(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if s <= 0.0 {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l[k * n + i] * x[k];
        }
        x[i] = s / l[i * n + i];
    }
    Some(x)
}

/// Kleiner, reproduzierbarer Zufallsgenerator (xorshift64*). Reproduzierbarkeit
/// ist hier wichtiger als Guete: dieselben Bilder sollen dasselbe Panorama geben.
#[derive(Clone)]
pub struct Zufall(u64);

impl Zufall {
    pub fn neu(saat: u64) -> Self {
        Zufall(saat.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn naechste(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn bis(&mut self, n: usize) -> usize {
        ((self.naechste() >> 11) % n as u64) as usize
    }
    /// Gleichverteilt in [0, 1).
    pub fn gleich(&mut self) -> f64 {
        (self.naechste() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Normalverteilt (Box-Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.gleich().max(1e-12);
        let u2 = self.gleich();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigenwerte_aufsteigend() {
        // Symmetrische Matrix mit bekannten Eigenwerten 1, 2, 4 (gedreht).
        let r = drehung(&[0.1, -0.3, 0.2]);
        let d = [[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 4.0]];
        let a = mul(&mul(&r, &d), &transp(&r));
        let flach: Vec<f64> = a.iter().flatten().copied().collect();
        let (w, v) = eigen_sym(&flach, 3);
        for (ist, soll) in w.iter().zip([1.0, 2.0, 4.0]) {
            assert!((ist - soll).abs() < 1e-9, "{w:?}");
        }
        // Eigenvektor zum kleinsten Eigenwert = erste Spalte von r (bis aufs Vorzeichen).
        let e = [v[0], v[3], v[6]];
        let c = (e[0] * r[0][0] + e[1] * r[1][0] + e[2] * r[2][0]).abs();
        assert!((c - 1.0).abs() < 1e-9, "{c}");
        assert!((det(&r) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn gleichungen() {
        let mut a = vec![4.0, 1.0, 0.0, 1.0, 3.0, 1.0, 0.0, 1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let x1 = cholesky_loesen(&a, &b, 3).unwrap();
        let x2 = loesen(&mut a, &mut b.clone(), 3).unwrap();
        for k in 0..3 {
            assert!((x1[k] - x2[k]).abs() < 1e-12);
        }
    }
}
