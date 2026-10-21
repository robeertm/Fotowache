//! Projektion der Sehstrahlen auf die Ausgabeflaeche.
//!
//! Welt: y zeigt nach unten, z in Blickrichtung der Panoramamitte (siehe
//! Horizontausgleich). `s` ist der Massstab in Pixel pro Radiant (Zylinder,
//! Kugel) bzw. Pixel pro Einheit in der Bildebene (Eben) — mit s = Brennweite
//! entspricht die Bildmitte jedes Fotos etwa seiner vollen Aufloesung.
//!
//! **Quer:** Eine senkrechte Reihe begradigt die Referenzumsetzung von Brown &
//! Lowe (Begradigung, senkrechte Art) so, dass sie auf der Projektion als
//! WAAGRECHTE Reihe liegt — der Schwenk laeuft dann entlang des Aequators, wo
//! Zylinder und Kugel am wenigsten verzerren, die Bilder liegen aber um 90 Grad
//! gedreht. `quer` dreht die Leinwand um 90 Grad zurueck (Leinwand (x, y) =
//! (−v, u) der gewoehnlichen Projektion): das Panorama steht aufrecht, die
//! Projektion bleibt die der Referenzumsetzung.

use super::mathe::{mal_vek, Mat3, Vek3};
use super::Projektion;
use std::f64::consts::PI;

/// Hoechster Winkel ueber/unter dem Horizont fuer Zylinder und Eben. Darueber
/// laufen beide Projektionen ins Unendliche (tan 90 Grad).
const MAX_WINKEL_ZYL: f64 = 1.25; // ~72 Grad
const MAX_WINKEL_EBEN: f64 = 1.35; // ~77 Grad

#[derive(Clone, Copy, Debug)]
pub struct Abbildung {
    pub proj: Projektion,
    pub s: f64,
    /// Leinwand um 90 Grad gedreht (senkrechte Reihe, siehe Modulkopf).
    pub quer: bool,
}

impl Abbildung {
    /// Strahl -> Leinwandstelle (u, v). `bezug`: gewuenschte Mitte fuer den
    /// Winkel der gewoehnlichen Projektion (damit ein Bild, das ueber die
    /// +-180-Grad-Grenze reicht, zusammenhaengend bleibt).
    pub fn vor(&self, d: &Vek3, bezug: f64) -> Option<(f64, f64)> {
        let (u, v) = self.vor_standard(d, bezug)?;
        Some(if self.quer { (-v, u) } else { (u, v) })
    }

    /// Strahl -> (u, v) der gewoehnlichen (nicht gedrehten) Projektion.
    fn vor_standard(&self, d: &Vek3, u_bezug: f64) -> Option<(f64, f64)> {
        let s = self.s;
        match self.proj {
            Projektion::Eben => {
                if d[2] <= 1e-9 {
                    return None;
                }
                Some((s * d[0] / d[2], s * d[1] / d[2]))
            }
            Projektion::Zylinder | Projektion::Kugel => {
                let mut th = d[0].atan2(d[2]);
                let bezug = u_bezug / s;
                while th - bezug > PI {
                    th -= 2.0 * PI;
                }
                while th - bezug < -PI {
                    th += 2.0 * PI;
                }
                let r = (d[0] * d[0] + d[2] * d[2]).sqrt();
                let v = if self.proj == Projektion::Zylinder {
                    if r < 1e-12 {
                        return None;
                    }
                    s * d[1] / r
                } else {
                    s * d[1].atan2(r)
                };
                Some((s * th, v))
            }
        }
    }

    /// Leinwandstelle (u, v) -> Strahl (nicht normiert).
    #[inline]
    pub fn zurueck(&self, u: f64, v: f64) -> Vek3 {
        let (u, v) = if self.quer { (v, -u) } else { (u, v) };
        let s = self.s;
        match self.proj {
            Projektion::Eben => [u / s, v / s, 1.0],
            Projektion::Zylinder => {
                let (sn, cs) = (u / s).sin_cos();
                [sn, v / s, cs]
            }
            Projektion::Kugel => {
                let (sn, cs) = (u / s).sin_cos();
                let (sp, cp) = (v / s).sin_cos();
                [cp * sn, sp, cp * cs]
            }
        }
    }

    /// Grenzen der Flaeche (umin, umax, vmin, vmax), die sinnvoll darstellbar ist.
    fn grenzen(&self) -> (f64, f64, f64, f64) {
        let s = self.s;
        match self.proj {
            Projektion::Eben => {
                let t = MAX_WINKEL_EBEN.tan() * s;
                (-t, t, -t, t)
            }
            Projektion::Zylinder => {
                let t = MAX_WINKEL_ZYL.tan() * s;
                (-3.0 * PI * s, 3.0 * PI * s, -t, t)
            }
            Projektion::Kugel => (-3.0 * PI * s, 3.0 * PI * s, -PI / 2.0 * s, PI / 2.0 * s),
        }
    }
}

/// Bildgeometrie eines Fotos in der Ausgabe.
#[derive(Clone, Copy, Debug)]
pub struct Kamera {
    pub r: Mat3,
    /// Brennweite und Hauptpunkt in Pixeln des jeweiligen Bildpuffers.
    pub f: f64,
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
    /// Radiale Verzeichnung (siehe `ausrichtung::Optik`) und ihr Bezugsradius
    /// (halbe Diagonale in Pixeln dieses Puffers).
    pub k1: f64,
    pub d: f64,
}

impl Kamera {
    /// Sehstrahl (Welt, nicht normiert) des Bildpunkts (x, y) — Pixelkoordinaten
    /// des Puffers, Pixelmitte bei +0,5.
    pub fn strahl(&self, x: f64, y: f64) -> Vek3 {
        let (u, v) = super::ausrichtung::entzeichnen(x - self.cx, y - self.cy, self.k1, self.d);
        mal_vek(&self.r, &[u, v, self.f])
    }

    /// Bildpunkt (Pixelkoordinaten, Mitte bei +0,5) eines Weltstrahls; `None`,
    /// wenn der Strahl hinter der Kamera liegt.
    pub fn bildpunkt(&self, d: &Vek3) -> Option<(f64, f64)> {
        let r = &self.r;
        let c = [
            r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2],
            r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2],
            r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2],
        ];
        if c[2] <= 1e-9 * (c[0].abs() + c[1].abs() + c[2].abs()) {
            return None;
        }
        let (x, y) = super::ausrichtung::verzeichnen(self.f * c[0] / c[2], self.f * c[1] / c[2], self.k1, self.d);
        Some((x + self.cx, y + self.cy))
    }
}

/// Umriss eines Bildes auf der Leinwand: Begrenzungsrechteck (umin, umax,
/// vmin, vmax).
pub fn umriss(k: &Kamera, a: &Abbildung) -> Option<(f64, f64, f64, f64)> {
    let e = umriss_standard(k, a)?;
    Some(if a.quer { (-e.3, -e.2, e.0, e.1) } else { e })
}

/// Umriss in der gewoehnlichen (nicht gedrehten) Projektion.
fn umriss_standard(k: &Kamera, a: &Abbildung) -> Option<(f64, f64, f64, f64)> {
    let strahl = |x: f64, y: f64| {
        let (u, v) = super::ausrichtung::entzeichnen(x - k.cx, y - k.cy, k.k1, k.d);
        mal_vek(&k.r, &[u, v, k.f])
    };
    let (u0, _) = a.vor_standard(&strahl(k.cx, k.cy), 0.0)?;
    let (mut umin, mut umax, mut vmin, mut vmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    const N: usize = 48;
    let mut nimm = |x: f64, y: f64| {
        if let Some((u, v)) = a.vor_standard(&strahl(x, y), u0) {
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
        }
    };
    for t in 0..=N {
        let q = t as f64 / N as f64;
        nimm(q * k.w, 0.0);
        nimm(q * k.w, k.h);
        nimm(0.0, q * k.h);
        nimm(k.w, q * k.h);
    }
    // Pol im Bild? Dann umfasst das Bild auf Zylinder/Kugel alle Laengen.
    if a.proj != Projektion::Eben {
        for pol in [[0.0, 1.0, 0.0], [0.0, -1.0, 0.0]] {
            let c = [
                k.r[0][0] * pol[0] + k.r[1][0] * pol[1] + k.r[2][0] * pol[2],
                k.r[0][1] * pol[0] + k.r[1][1] * pol[1] + k.r[2][1] * pol[2],
                k.r[0][2] * pol[0] + k.r[1][2] * pol[1] + k.r[2][2] * pol[2],
            ];
            if c[2] > 0.0 {
                let x = k.f * c[0] / c[2] + k.cx;
                let y = k.f * c[1] / c[2] + k.cy;
                if x >= 0.0 && x <= k.w && y >= 0.0 && y <= k.h {
                    umin = u0 - PI * a.s;
                    umax = u0 + PI * a.s;
                    if pol[1] > 0.0 {
                        vmax = f64::MAX;
                    } else {
                        vmin = f64::MIN;
                    }
                }
            }
        }
    }
    let (gu0, gu1, gv0, gv1) = a.grenzen();
    let e = (umin.max(gu0), umax.min(gu1), vmin.max(gv0), vmax.min(gv1));
    if e.0 >= e.1 || e.2 >= e.3 {
        return None;
    }
    Some(e)
}

/// Ausgabeflaeche: Ursprung (u0, v0) und Groesse in Pixeln.
#[derive(Clone, Copy, Debug)]
pub struct Leinwand {
    pub a: Abbildung,
    pub u0: f64,
    pub v0: f64,
    pub w: usize,
    pub h: usize,
}

impl Leinwand {
    /// Die Leinwand im Massstab `faktor` (fuer die kleine Nahtsuch-Leinwand).
    pub fn skaliert(&self, faktor: f64) -> Leinwand {
        Leinwand {
            a: Abbildung { proj: self.a.proj, s: self.a.s * faktor, quer: self.a.quer },
            u0: self.u0 * faktor,
            v0: self.v0 * faktor,
            w: ((self.w as f64 * faktor).ceil() as usize).max(1),
            h: ((self.h as f64 * faktor).ceil() as usize).max(1),
        }
    }

    /// Leinwandstelle (Pixel, Ursprung oben links) eines Weltstrahls.
    pub fn stelle(&self, d: &Vek3) -> Option<(f64, f64)> {
        // Bezug des Winkels: Mitte der Leinwand (in der gewoehnlichen Projektion).
        let bezug = if self.a.quer { self.v0 + self.h as f64 / 2.0 } else { self.u0 + self.w as f64 / 2.0 };
        let (u, v) = self.a.vor(d, bezug)?;
        Some((u - self.u0, v - self.v0))
    }

    /// Pixelrechteck [x0, x1) x [y0, y1) eines Bildes auf dieser Leinwand.
    pub fn rechteck(&self, k: &Kamera) -> Option<(usize, usize, usize, usize)> {
        let (umin, umax, vmin, vmax) = umriss(k, &self.a)?;
        let x0 = ((umin - self.u0).floor().max(0.0)) as usize;
        let y0 = ((vmin - self.v0).floor().max(0.0)) as usize;
        let x1 = ((umax - self.u0).ceil().max(0.0) as usize + 1).min(self.w);
        let y1 = ((vmax - self.v0).ceil().max(0.0) as usize + 1).min(self.h);
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        Some((x0, x1, y0, y1))
    }
}

/// Legt die Leinwand fest: Vereinigung aller Umrisse, notfalls verkleinert, damit
/// die laengere Seite `max_seite` und die Flaeche `max_pixel` nicht ueberschreitet.
pub fn leinwand(kameras: &[Kamera], proj: Projektion, s: f64, quer: bool, max_seite: u32, max_pixel: f64) -> Option<(Leinwand, bool)> {
    let a = Abbildung { proj, s, quer };
    let (mut umin, mut umax, mut vmin, mut vmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    let mut beschnitten = false;
    for k in kameras {
        let (a0, a1, b0, b1) = umriss_standard(k, &a)?;
        umin = umin.min(a0);
        umax = umax.max(a1);
        vmin = vmin.min(b0);
        vmax = vmax.max(b1);
        let (gu0, gu1, gv0, gv1) = a.grenzen();
        if a0 <= gu0 + 1.0 || a1 >= gu1 - 1.0 || b0 <= gv0 + 1.0 || b1 >= gv1 - 1.0 {
            beschnitten = true;
        }
    }
    // Rundum: nie mehr als einmal herum.
    if proj != Projektion::Eben && umax - umin > 2.0 * PI * s {
        let mitte = 0.5 * (umax + umin);
        umin = mitte - PI * s;
        umax = mitte + PI * s;
    }
    let breite = umax - umin;
    let hoehe = vmax - vmin;
    let mut faktor = 1.0f64;
    let seite = breite.max(hoehe);
    if seite > max_seite as f64 {
        faktor = faktor.min(max_seite as f64 / seite);
    }
    if breite * hoehe * faktor * faktor > max_pixel {
        faktor = (max_pixel / (breite * hoehe)).sqrt();
    }
    let s2 = s * faktor;
    let a = Abbildung { proj, s: s2, quer };
    let w = ((breite * faktor).ceil() as usize).max(1);
    let h = ((hoehe * faktor).ceil() as usize).max(1);
    let l = if quer {
        // Leinwand (x, y) = (−v, u): x von −vmax bis −vmin, y von umin bis umax.
        Leinwand { a, u0: -vmax * faktor, v0: umin * faktor, w: h, h: w }
    } else {
        Leinwand { a, u0: umin * faktor, v0: vmin * faktor, w, h }
    };
    Some((l, beschnitten))
}
