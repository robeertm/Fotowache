//! Pruefungen mit synthetischen Szenen: bekannte Geometrie und bekannte
//! Helligkeitsunterschiede, Eingabe in vertauschter Reihenfolge. Gemessen wird
//! gegen die Wahrheit, nicht gegen den eigenen Bericht.

use super::ausrichtung::{uebertragen, Optik};
use super::mathe::{drehung, mal_vek, mul, Mat3, Zufall};
use super::naht::abtasten;
use super::*;

/// Strukturierte Szene: mehrstufiges Wertrauschen plus zufaellige Flecken und
/// Rechtecke (liefert Ecken in allen Groessen), linear 0.02..0.8.
pub(super) fn szene(w: usize, h: usize, saat: u64) -> Vec<f32> {
    let mut z = Zufall::neu(saat);
    let mut d = vec![0.0f32; w * h * 3];
    for (abstand, amp) in [(96usize, 0.35f32), (40, 0.25), (16, 0.15), (8, 0.08)] {
        let gw = w / abstand + 3;
        let gh = h / abstand + 3;
        let gitter: Vec<[f32; 3]> = (0..gw * gh).map(|_| [z.gleich() as f32, z.gleich() as f32, z.gleich() as f32]).collect();
        for y in 0..h {
            let fy = y as f32 / abstand as f32;
            let iy = fy as usize;
            let ty = fy - iy as f32;
            let ty = ty * ty * (3.0 - 2.0 * ty);
            for x in 0..w {
                let fx = x as f32 / abstand as f32;
                let ix = fx as usize;
                let tx = fx - ix as f32;
                let tx = tx * tx * (3.0 - 2.0 * tx);
                for c in 0..3 {
                    let a = gitter[iy * gw + ix][c] * (1.0 - tx) + gitter[iy * gw + ix + 1][c] * tx;
                    let b = gitter[(iy + 1) * gw + ix][c] * (1.0 - tx) + gitter[(iy + 1) * gw + ix + 1][c] * tx;
                    d[(y * w + x) * 3 + c] += amp * (a * (1.0 - ty) + b * ty);
                }
            }
        }
    }
    let formen = w * h / 2500;
    for _ in 0..formen {
        let cx = z.gleich() * w as f64;
        let cy = z.gleich() * h as f64;
        let r = 4.0 + z.gleich() * 26.0;
        let farbe = [z.gleich() as f32, z.gleich() as f32, z.gleich() as f32];
        let rund = z.gleich() < 0.5;
        let (x0, x1) = ((cx - r).max(0.0) as usize, ((cx + r) as usize).min(w - 1));
        let (y0, y1) = ((cy - r).max(0.0) as usize, ((cy + r) as usize).min(h - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                if rund && dx * dx + dy * dy > r * r {
                    continue;
                }
                for c in 0..3 {
                    d[(y * w + x) * 3 + c] = 0.5 * d[(y * w + x) * 3 + c] + 0.5 * farbe[c];
                }
            }
        }
    }
    // Leicht glaetten (gegen Treppen an den Formkanten) und in den Bereich bringen.
    for v in d.iter_mut() {
        *v = 0.02 + 0.78 * (*v / 1.2).clamp(0.0, 1.0);
    }
    d
}

/// Ansicht einer Szene, die selbst als Lochkamerabild (Brennweite f0) gilt:
/// Drehung `r` (Kamera -> Welt), Brennweite f1, Groesse vw x vh, Verstaerkung c,
/// radiale Verzeichnung k1 (wie im Modell, bezogen auf die halbe Diagonale).
/// 2x2-Ueberabtastung gegen Treppen und Aliasing.
#[allow(clippy::too_many_arguments)]
fn ansicht(sz: &[f32], sw: usize, sh: usize, f0: f64, r: &Mat3, f1: f64, vw: usize, vh: usize, c: f32, k1: f64) -> Vec<f32> {
    let halb_diag = (vw as f64).hypot(vh as f64) / 2.0;
    let mut d = vec![0.0f32; vw * vh * 3];
    for y in 0..vh {
        for x in 0..vw {
            let mut s = [0.0f32; 3];
            for (oy, ox) in [(0.25, 0.25), (0.25, 0.75), (0.75, 0.25), (0.75, 0.75)] {
                let (u, v) = super::ausrichtung::entzeichnen(x as f64 + ox - vw as f64 / 2.0, y as f64 + oy - vh as f64 / 2.0, k1, halb_diag);
                let w = mal_vek(r, &[u, v, f1]);
                let px = f0 * w[0] / w[2] + sw as f64 / 2.0;
                let py = f0 * w[1] / w[2] + sh as f64 / 2.0;
                let p = abtasten(sz, sw, sh, px as f32, py as f32);
                for k in 0..3 {
                    s[k] += 0.25 * p[k];
                }
            }
            for k in 0..3 {
                d[(y * vw + x) * 3 + k] = s[k] * c;
            }
        }
    }
    d
}

fn dreh_yp(gier: f64, nick: f64) -> Mat3 {
    // erst Nicken (um x), dann Gieren (um y): R = Ry * Rx
    mul(&drehung(&[0.0, gier.to_radians(), 0.0]), &drehung(&[-nick.to_radians(), 0.0, 0.0]))
}

struct Fall {
    bilder: Vec<Eingabe>,
    /// Wahrheit je Eingabe: (Drehung, Verstaerkung)
    wahr: Vec<(Mat3, f32)>,
    f1: f64,
    k1: f64,
    /// halbe Bilddiagonale (Bezug der Verzeichnung)
    d: f64,
    /// Brennweite je Eingabe (Zoom/Standortwechsel)
    f_je: Vec<f64>,
}

fn drehfall(szene_saat: u64, lagen: &[(f64, f64)], gains: &[f32], reihenfolge: &[usize]) -> Fall {
    drehfall_k1(szene_saat, lagen, gains, reihenfolge, 0.0)
}

fn drehfall_k1(szene_saat: u64, lagen: &[(f64, f64)], gains: &[f32], reihenfolge: &[usize], k1: f64) -> Fall {
    drehfall_voll(szene_saat, lagen, gains, reihenfolge, k1, &vec![1.0; lagen.len()])
}

#[allow(clippy::too_many_arguments)]
fn drehfall_voll(szene_saat: u64, lagen: &[(f64, f64)], gains: &[f32], reihenfolge: &[usize], k1: f64, zoom: &[f64]) -> Fall {
    let (sw, sh, f0) = (3000usize, 1600usize, 700.0);
    let sz = szene(sw, sh, szene_saat);
    let (vw, vh, f1) = (480usize, 360usize, 560.0);
    let mut bilder = Vec::new();
    let mut wahr = Vec::new();
    let mut f_je = Vec::new();
    for &k in reihenfolge {
        let r = dreh_yp(lagen[k].0, lagen[k].1);
        let rgb = ansicht(&sz, sw, sh, f0, &r, f1 * zoom[k], vw, vh, gains[k], k1);
        f_je.push(f1 * zoom[k]);
        bilder.push(Eingabe { w: vw as u32, h: vh as u32, rgb, brennweite_px: None, entzerrt: false });
        wahr.push((r, gains[k]));
    }
    Fall { bilder, wahr, f1, k1, d: (vw as f64).hypot(vh as f64) / 2.0, f_je }
}

/// Mittlerer und groesster Fehler der Abbildung zwischen allen ueberlappenden
/// Paaren verwendeter Bilder, gegen die Wahrheit (Pixel).
fn ausrichtungsfehler(fall: &Fall, e: &Ergebnis) -> (f64, f64) {
    let m = e;
    let (mut s, mut n, mut max) = (0.0, 0usize, 0.0f64);
    for (a, &ia) in e.verwendet.iter().enumerate() {
        for (b, &ib) in e.verwendet.iter().enumerate() {
            if a == b {
                continue;
            }
            let (ka, kb) = (&m.kameras[a], &m.kameras[b]);
            let (wa, wb) = (&fall.wahr[ia].0, &fall.wahr[ib].0);
            let (w, h) = (fall.bilder[ia].w as f64, fall.bilder[ia].h as f64);
            for gy in 1..8 {
                for gx in 1..10 {
                    let x = gx as f64 / 10.0 * w - w / 2.0;
                    let y = gy as f64 / 8.0 * h - h / 2.0;
                    let Some(soll) = uebertragen(wa, wb, Optik { f: fall.f_je[ia], k1: fall.k1 }, Optik { f: fall.f_je[ib], k1: fall.k1 }, fall.d, fall.d, x, y) else { continue };
                    let wb_ = fall.bilder[ib].w as f64 / 2.0 - 2.0;
                    let hb_ = fall.bilder[ib].h as f64 / 2.0 - 2.0;
                    if soll.0.abs() > wb_ || soll.1.abs() > hb_ {
                        continue;
                    }
                    let ist = uebertragen(&ka.r, &kb.r, Optik { f: ka.f, k1: ka.k1 }, Optik { f: kb.f, k1: kb.k1 }, ka.d, kb.d, x, y).expect("Rueckprojektion");
                    let d = ((ist.0 - soll.0).powi(2) + (ist.1 - soll.1).powi(2)).sqrt();
                    s += d;
                    n += 1;
                    max = max.max(d);
                }
            }
        }
    }
    assert!(n > 20, "zu wenig Pruefpunkte: {n}");
    (s / n as f64, max)
}

/// Groesste relative Abweichung von g_i * c_i vom Mittel (0 = vollstaendig
/// angeglichen). Mit `ohne_ausgleich` die Abweichung der c_i selbst.
fn gain_streuung_von(fall: &Fall, e: &Ergebnis, ohne_ausgleich: bool) -> f64 {
    let p: Vec<f64> = e
        .verwendet
        .iter()
        .zip(&e.verstaerkungen)
        .map(|(&k, g)| if ohne_ausgleich { 1.0 } else { *g as f64 } * fall.wahr[k].1 as f64)
        .collect();
    let m = p.iter().sum::<f64>() / p.len() as f64;
    p.iter().map(|v| (v / m - 1.0).abs()).fold(0.0, f64::max)
}

/// Belichtungsausgleich nach B&L Gl. 29: der Zug zu 1 (σg = 0,1) laesst mit
/// Absicht einen Rest stehen (die Mehrband-Mischung glaettet ihn). Gemessen an
/// den Pruefszenen: Rest 0,20 (einzeilig) bzw. 0,26 (Raster) der Ausgangsstreuung;
/// geprueft wird < 0,3.
fn gain_streuung(fall: &Fall, e: &Ergebnis) -> f64 {
    let (vor, nach) = (gain_streuung_von(fall, e, true), gain_streuung_von(fall, e, false));
    eprintln!("Belichtung: Abweichung vorher {:.2} %, nachher {:.2} %", vor * 100.0, nach * 100.0);
    nach / vor
}

fn still(_: f32, _: Schritt) {}

#[test]
fn einzeilig_vertauscht_mit_belichtung() {
    let lagen = [(-30.0, 0.0), (-10.0, 0.0), (10.0, 0.0), (30.0, 0.0)];
    let gains = [1.0, 0.75, 1.3, 0.9];
    let fall = drehfall(11, &lagen, &gains, &[2, 0, 3, 1]);
    let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet.len(), 4, "{}", bericht_deutsch(&e.bericht));
    let (mittel, max) = ausrichtungsfehler(&fall, &e);
    eprintln!("Fehler gegen Wahrheit: mittel {mittel:.3} px, max {max:.3} px");
    assert!(mittel < 0.5 && max < 1.5, "mittel {mittel}, max {max}");
    assert!((e.brennweite_px as f64 - fall.f1).abs() / fall.f1 < 0.01, "f {}", e.brennweite_px);
    let st = gain_streuung(&fall, &e);
    eprintln!("Belichtung: groesste Abweichung {:.2} %", st * 100.0);
    assert!(st < 0.3, "Belichtungsausgleich {st}");
    // Horizont: waagrechte Kameraachsen muessen waagrecht liegen.
    for k in &e.kameras {
        assert!(k.r[1][0].abs() < 0.005, "Horizont schief: {:?}", k.r);
    }
    // Breite etwa 60 Grad Schwenk + Bildwinkel, Hoehe etwa ein Bild.
    assert!(e.w > 900 && e.w < 1200 && e.h > 340 && e.h < 420, "{} x {}", e.w, e.h);
    let r = groesstes_rechteck(&e.deckung, e.w, e.h);
    assert!((r[2] - r[0]) as f64 > 0.85 * e.w as f64, "{r:?}");
}

/// Objektiv mit Tonnenverzeichnung (4 % in den Ecken): ohne Verzeichnungsmodell
/// klaffen die Bildraender mehrere Pixel auseinander.
#[test]
fn verzeichnung_wird_ausgeglichen() {
    let lagen = [(-24.0, 0.0), (-8.0, 0.0), (8.0, 0.0), (24.0, 0.0)];
    let fall = drehfall_k1(13, &lagen, &[1.0, 0.9, 1.1, 1.0], &[1, 3, 0, 2], -0.04);
    let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet.len(), 4, "{}", bericht_deutsch(&e.bericht));
    let (mittel, max) = ausrichtungsfehler(&fall, &e);
    eprintln!("Verzeichnung: k1 {:.4} (wahr -0.04), Fehler mittel {mittel:.3} px, max {max:.3} px", e.verzeichnung_k1);
    assert!((e.verzeichnung_k1 as f64 + 0.04).abs() < 0.004, "k1 {}", e.verzeichnung_k1);
    assert!(mittel < 0.5 && max < 1.5, "mittel {mittel}, max {max}");
    assert!((e.brennweite_px as f64 - fall.f1).abs() / fall.f1 < 0.01, "f {}", e.brennweite_px);
}

/// Ein Bild mit anderer Brennweite (gezoomt oder naeher herangegangen):
/// Merkmale muessen ueber den Massstab hinweg gefunden und der Sprung als
/// Bildfaktor ausgeglichen werden.
#[test]
fn massstabssprung_wird_ausgeglichen() {
    let lagen = [(-20.0, 0.0), (0.0, 0.0), (18.0, 0.0)];
    let fall = drehfall_voll(17, &lagen, &[1.0, 1.0, 1.0], &[2, 0, 1], 0.0, &[1.0, 1.0, 1.45]);
    let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet.len(), 3, "{}", bericht_deutsch(&e.bericht));
    let (mittel, max) = ausrichtungsfehler(&fall, &e);
    eprintln!("Massstabssprung: Fehler mittel {mittel:.3} px, max {max:.3} px");
    assert!(mittel < 0.5 && max < 1.5, "mittel {mittel}, max {max}");
    assert!(bericht_deutsch(&e.bericht).contains("Hinweis: Bild 1 erscheint 1,45-fach größer"), "{}", bericht_deutsch(&e.bericht));
}

#[test]
fn raster_zwei_zeilen() {
    let lagen = [(-22.0, -12.0), (0.0, -12.0), (22.0, -12.0), (-22.0, 12.0), (0.0, 12.0), (22.0, 12.0)];
    let gains = [1.0, 1.1, 0.9, 0.85, 1.0, 1.2];
    let fall = drehfall(23, &lagen, &gains, &[4, 1, 5, 0, 3, 2]);
    let opt = Optionen { projektion: Projektion::Kugel, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet.len(), 6, "{}", bericht_deutsch(&e.bericht));
    let (mittel, max) = ausrichtungsfehler(&fall, &e);
    eprintln!("Raster gegen Wahrheit: mittel {mittel:.3} px, max {max:.3} px");
    assert!(mittel < 0.6 && max < 2.0, "mittel {mittel}, max {max}");
    assert!(gain_streuung(&fall, &e) < 0.3);
    assert!(e.h as f64 > 0.5 * e.w as f64, "{} x {}", e.w, e.h);
}

#[test]
fn senkrechte_reihe() {
    let lagen = [(0.0, -25.0), (0.0, 0.0), (0.0, 25.0)];
    let fall = drehfall(31, &lagen, &[1.0, 1.0, 1.0], &[1, 2, 0]);
    let opt = Optionen { projektion: Projektion::Kugel, belichtung_angleichen: false, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    assert_eq!(e.verwendet.len(), 3, "{}", bericht_deutsch(&e.bericht));
    let (mittel, _) = ausrichtungsfehler(&fall, &e);
    assert!(mittel < 0.6, "{mittel}");
    // Hochformat, und die Reihe steht senkrecht (nicht um 90 Grad gekippt).
    assert!(e.h > e.w, "{} x {}", e.w, e.h);
}

#[test]
fn fremdes_bild_wird_verworfen() {
    let lagen = [(-20.0, 0.0), (0.0, 0.0), (20.0, 0.0)];
    let mut fall = drehfall(41, &lagen, &[1.0, 1.0, 1.0], &[0, 1, 2]);
    let fremd = drehfall(99, &[(0.0, 0.0)], &[1.0], &[0]);
    fall.bilder.insert(1, fremd.bilder.into_iter().next().unwrap());
    fall.wahr.insert(1, fremd.wahr[0]);
    let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet, vec![0, 2, 3]);
    assert!(bericht_deutsch(&e.bericht).contains("Verworfen: Bild 2"), "{}", bericht_deutsch(&e.bericht));
}

#[test]
fn nichts_passt() {
    let a = drehfall(5, &[(0.0, 0.0)], &[1.0], &[0]);
    let b = drehfall(6, &[(0.0, 0.0)], &[1.0], &[0]);
    let bilder: Vec<Eingabe> = a.bilder.into_iter().chain(b.bilder).collect();
    let r = zusammensetzen(&bilder, &Optionen::default(), &still);
    assert!(r.is_err());
}

/// Ebene Ausschnitte (Verschiebung + leichte Drehung in der Bildebene, wie aus
/// einem grossen Foto geschnitten): das entspricht einer sehr langen Brennweite.
/// Mit Projektion "Eben" muss das Ergebnis die Ausgangsszene wiedergeben —
/// nahtlos und gleich hell.
#[test]
fn ebene_ausschnitte_ergeben_die_szene() {
    let (sw, sh) = (1500usize, 700usize);
    let sz = szene(sw, sh, 77);
    // (Mitte x, Mitte y, Drehung Grad, Verstaerkung). Gleiche Belichtung: hier
    // wird die Nahtlosigkeit gegen die Szene gemessen; den Belichtungsausgleich
    // (der nach B&L Gl. 29 mit Absicht einen Rest laesst) pruefen die anderen
    // Faelle.
    let teile: [(f64, f64, f64, f32); 4] = [(300.0, 350.0, 0.0, 1.0), (600.0, 340.0, 2.0, 1.0), (900.0, 365.0, -1.5, 1.0), (1200.0, 345.0, 1.0, 1.0)];
    let (vw, vh) = (480usize, 480usize);
    let reihenfolge = [3usize, 1, 0, 2];
    let mut bilder = Vec::new();
    for &k in &reihenfolge {
        let (mx, my, wi, c) = teile[k];
        let (sn, cs) = wi.to_radians().sin_cos();
        let mut d = vec![0.0f32; vw * vh * 3];
        for y in 0..vh {
            for x in 0..vw {
                let (ox, oy) = (x as f64 + 0.5 - vw as f64 / 2.0, y as f64 + 0.5 - vh as f64 / 2.0);
                let px = mx + cs * ox - sn * oy;
                let py = my + sn * ox + cs * oy;
                let p = abtasten(&sz, sw, sh, px as f32, py as f32);
                for q in 0..3 {
                    d[(y * vw + x) * 3 + q] = p[q] * c;
                }
            }
        }
        bilder.push(Eingabe { w: vw as u32, h: vh as u32, rgb: d, brennweite_px: None, entzerrt: false });
    }
    let opt = Optionen { projektion: Projektion::Eben, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&bilder, &opt, &still).expect("Panorama");
    let m = &e;
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet.len(), 4);
    // Gain-Pruefung
    let p: Vec<f64> = e.verwendet.iter().zip(&e.verstaerkungen).map(|(&k, g)| *g as f64 * teile[reihenfolge[k]].3 as f64).collect();
    let mittel = p.iter().sum::<f64>() / p.len() as f64;
    let st = p.iter().map(|v| (v / mittel - 1.0).abs()).fold(0.0, f64::max);
    eprintln!("Ausschnitte: Belichtung groesste Abweichung {:.2} %", st * 100.0);
    assert!(st < 0.02, "{st}");
    // Ausrichtung gegen Wahrheit: Punkt in Teil a -> Szene -> Teil b.
    let teil_zu_szene = |k: usize, x: f64, y: f64| {
        let (mx, my, wi, _) = teile[k];
        let (sn, cs) = wi.to_radians().sin_cos();
        (mx + cs * x - sn * y, my + sn * x + cs * y)
    };
    let szene_zu_teil = |k: usize, sx: f64, sy: f64| {
        let (mx, my, wi, _) = teile[k];
        let (sn, cs) = wi.to_radians().sin_cos();
        let (dx, dy) = (sx - mx, sy - my);
        (cs * dx + sn * dy, -sn * dx + cs * dy)
    };
    let (mut s, mut n, mut max) = (0.0, 0, 0.0f64);
    for (a, &ia) in e.verwendet.iter().enumerate() {
        for (b, &ib) in e.verwendet.iter().enumerate() {
            if a == b {
                continue;
            }
            for gy in 1..8 {
                for gx in 1..8 {
                    let (x, y) = (gx as f64 / 8.0 * 480.0 - 240.0, gy as f64 / 8.0 * 480.0 - 240.0);
                    let (sx, sy) = teil_zu_szene(reihenfolge[ia], x, y);
                    let soll = szene_zu_teil(reihenfolge[ib], sx, sy);
                    if soll.0.abs() > 236.0 || soll.1.abs() > 236.0 {
                        continue;
                    }
                    let (ka, kb) = (&m.kameras[a], &m.kameras[b]);
                    let ist = uebertragen(&ka.r, &kb.r, Optik { f: ka.f, k1: ka.k1 }, Optik { f: kb.f, k1: kb.k1 }, ka.d, kb.d, x, y).unwrap();
                    let d = ((ist.0 - soll.0).powi(2) + (ist.1 - soll.1).powi(2)).sqrt();
                    s += d;
                    n += 1;
                    max = max.max(d);
                }
            }
        }
    }
    let mittel_f = s / n as f64;
    eprintln!("Ausschnitte gegen Wahrheit: mittel {mittel_f:.3} px, max {max:.3} px ({n} Punkte)");
    assert!(mittel_f < 0.5 && max < 1.5, "{mittel_f} {max}");

    // Nahtlosigkeit: Ausgabe gegen die Szene, in 16x16-Bloecken. Jedes
    // Ausgabepixel wird ueber das naechstgelegene Bild, das es abdeckt, auf die
    // Szene zurueckgefuehrt (so gilt die Referenz ueberall gleich genau).
    // Getrennt gemessen fuer Bloecke mit einem Bild (Grundrauschen der doppelten
    // Abtastung) und Bloecke in Ueberlappungen (dort liegen die Naehte): die
    // Ueberlappung darf nicht nennenswert schlechter sein.
    let l = &m.leinwand;
    let faktor = mittel; // mittleres g_i * c_i
    let b = 16usize;
    let mut bloecke: Vec<(f64, f64, bool)> = Vec::new(); // (Ausgabe, Szene, Ueberlappung)
    for by in 0..(e.h as usize / b) {
        'block: for bx in 0..(e.w as usize / b) {
            let (mut so, mut ss) = (0.0f64, 0.0f64);
            let mut mehrfach = false;
            for y in by * b..(by + 1) * b {
                for x in bx * b..(bx + 1) * b {
                    let i = y * e.w as usize + x;
                    if e.deckung[i] == 0 {
                        continue 'block;
                    }
                    let mut naechst: Option<(f64, usize, f64, f64)> = None;
                    let mut gedeckt = 0;
                    for (a, k) in m.kameras.iter().enumerate() {
                        let Some((u, v)) = super::naht::quelle(l, k, x, y) else { continue };
                        if u <= 0.0 || v <= 0.0 || u >= k.w || v >= k.h {
                            continue;
                        }
                        gedeckt += 1;
                        let d = (u - k.cx).powi(2) + (v - k.cy).powi(2);
                        if naechst.is_none_or(|n| d < n.0) {
                            naechst = Some((d, a, u - k.cx, v - k.cy));
                        }
                    }
                    let Some((_, a, u, v)) = naechst else { continue 'block };
                    mehrfach |= gedeckt > 1;
                    let (sx, sy) = teil_zu_szene(reihenfolge[e.verwendet[a]], u, v);
                    if sx < 2.0 || sy < 2.0 || sx > sw as f64 - 2.0 || sy > sh as f64 - 2.0 {
                        continue 'block;
                    }
                    let p = abtasten(&sz, sw, sh, sx as f32, sy as f32);
                    let o = &e.rgb[i * 3..i * 3 + 3];
                    for c in 0..3 {
                        so += o[c] as f64;
                        ss += p[c] as f64 * faktor;
                    }
                }
            }
            bloecke.push((so, ss, mehrfach));
        }
    }
    let pegel = bloecke.iter().map(|b| b.1).sum::<f64>() / bloecke.len() as f64;
    let mass = |ueber: bool| {
        let f: Vec<f64> = bloecke.iter().filter(|b| b.2 == ueber).map(|b| (b.0 - b.1).abs() / pegel).collect();
        (f.len(), f.iter().sum::<f64>() / f.len().max(1) as f64, f.iter().cloned().fold(0.0, f64::max))
    };
    let (n1, m1, x1) = mass(false);
    let (n2, m2, x2) = mass(true);
    eprintln!(
        "Ausschnitte gegen Szene: Einzelbild {n1} Bloecke, mittel {:.2} %, max {:.2} % | Ueberlappung {n2} Bloecke, mittel {:.2} %, max {:.2} %",
        m1 * 100.0, x1 * 100.0, m2 * 100.0, x2 * 100.0
    );
    assert!(n1 > 300 && n2 > 300, "{n1} {n2}");
    assert!(m2 < 0.005 && m2 < 1.5 * m1 + 0.001, "Ueberlappung {m2} gegen Einzelbild {m1}");
    assert!(x2 < 0.04, "groesste Abweichung in der Ueberlappung {x2}");
}

#[test]
fn rechteck_in_deckung() {
    // Raute-aehnliche Deckung mit bekanntem groessten Rechteck.
    let (w, h) = (40usize, 30usize);
    let mut d = vec![0u8; w * h];
    for y in 5..25 {
        for x in 3..37 {
            d[y * w + x] = 255;
        }
    }
    // Zacken oben und eine Luecke
    for x in 10..14 {
        for y in 2..5 {
            d[y * w + x] = 255;
        }
    }
    d[12 * w + 20] = 0;
    let r = groesstes_rechteck(&d, w as u32, h as u32);
    // Groesstes: unter der Luecke y 13..25, x 3..37 (34 x 12 = 408) gegen
    // ueber der Luecke y 5..12 (34 x 7) oder links/rechts der Luecke
    // (x 21..37 x y 5..25 = 16 x 20 = 320; x 3..20 = 17 x 20 = 340).
    assert_eq!(r, [3, 13, 37, 25]);
    assert_eq!(groesstes_rechteck(&[0u8; 4], 2, 2), [0, 0, 0, 0]);
}


/// Elf Bilder in einer Reihe (frueher Absturz: Index ausserhalb, sobald nach
/// dem Herausnehmen von Paaren die Spannbaum-Kanten auf die alte Paarliste
/// zeigten). Alle elf muessen verwendet werden, die Lage muss stimmen.
#[test]
fn elf_bilder_ohne_absturz() {
    let lagen: Vec<(f64, f64)> = (0..11).map(|k| (-36.0 + 7.2 * k as f64, 0.0)).collect();
    let gains = vec![1.0f32; 11];
    let reihenfolge: Vec<usize> = (0..11).map(|k| (k * 7 + 3) % 11).collect();
    let fall = drehfall(53, &lagen, &gains, &reihenfolge);
    let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: true };
    let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
    eprintln!("{}", bericht_deutsch(&e.bericht));
    assert_eq!(e.verwendet.len(), 11, "{}", bericht_deutsch(&e.bericht));
    let (mittel, max) = ausrichtungsfehler(&fall, &e);
    eprintln!("Elf Bilder gegen Wahrheit: mittel {mittel:.3} px, max {max:.3} px");
    assert!(mittel < 0.5 && max < 1.5, "mittel {mittel}, max {max}");
}

// ---------------------------------------------------------------------------
// Parallaxe (Teil B, Zhang & Liu 2014)

/// Ansicht einer schraegen, nahen Wand (Ebene z = 3 + 0,8 x) von einem um `c`
/// verschobenen Standpunkt: die Tiefe aendert sich ueber das Bild stetig, wie
/// bei einer Hausfront schraeg zur Blickrichtung. 2x2-Ueberabtastung.
#[allow(clippy::too_many_arguments)]
fn schraege_ansicht(textur: &[f32], tw: usize, th: usize, c: [f64; 3], r: &Mat3, f1: f64, w: usize, h: usize) -> Vec<f32> {
    let k = 600.0; // Texturpixel je Einheit
    let mut d = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let mut s = [0.0f32; 3];
            for (oy, ox) in [(0.25, 0.25), (0.25, 0.75), (0.75, 0.25), (0.75, 0.75)] {
                let dir = mal_vek(r, &[x as f64 + ox - w as f64 / 2.0, y as f64 + oy - h as f64 / 2.0, f1]);
                let t = (3.0 + 0.8 * c[0] - c[2]) / (dir[2] - 0.8 * dir[0]);
                let (px, py) = (c[0] + t * dir[0], c[1] + t * dir[1]);
                let p = abtasten(textur, tw, th, (px * k + tw as f64 / 2.0) as f32, (py * k + th as f64 / 2.0) as f32);
                for q in 0..3 {
                    s[q] += 0.25 * p[q];
                }
            }
            d[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&s);
        }
    }
    d
}

/// Parallaxe: zwei Aufnahmen einer nahen, schraegen Wand mit seitlich
/// versetztem Projektionszentrum (wie freihaendig, ohne Nodalpunkt). Das
/// Drehmodell kann das nicht abbilden (Restversatz ueber der Schwelle). Gemessen
/// wird gegen die WAHRHEIT: jede Nahtstelle wird ueber Bild A auf die Wand
/// zurueckgerechnet und in Bild B projiziert; die Abweichung von der Stelle, an
/// der das Panorama Bild B dort abtastet, ist der Versatz an der Naht. Mit
/// Ausgleich (Zhang & Liu) muss er deutlich kleiner werden.
#[test]
fn parallaxe_verringert_den_versatz_an_der_naht() {
    let (tw, th) = (4000usize, 3000usize);
    let textur = szene(tw, th, 63);
    let (w, h, f1) = (1440usize, 1080usize, 1680.0);
    // 1440 x 1080: bei kleinen Bildern waere 1 px Restversatz (die Schwelle)
    // schon so viel Parallaxe, dass Z&L jede Homographie als zu perspektivisch
    // verwirft (Gl. 3) — dieselbe Lage wie bei echten Fotos braucht echte Groesse.
    let lagen = [(-8.0, -0.02), (8.0, 0.02)];
    let bilder: Vec<Eingabe> = lagen
        .iter()
        .map(|&(gier, cx)| Eingabe { w: w as u32, h: h as u32, rgb: schraege_ansicht(&textur, tw, th, [cx, 0.0, 0.0], &dreh_yp(gier, 0.0), f1, w, h), brennweite_px: None, entzerrt: false })
        .collect();
    // Wahrheit: Bildpunkt in A -> Wandpunkt -> Bildpunkt in B.
    let wahr = |x: f64, y: f64| -> (f64, f64) {
        let (ra, rb) = (dreh_yp(lagen[0].0, 0.0), dreh_yp(lagen[1].0, 0.0));
        let (ca, cb) = ([lagen[0].1, 0.0, 0.0], [lagen[1].1, 0.0, 0.0]);
        let d = mal_vek(&ra, &[x - w as f64 / 2.0, y - h as f64 / 2.0, f1]);
        let t = (3.0 + 0.8 * ca[0] - ca[2]) / (d[2] - 0.8 * d[0]);
        let p = [ca[0] + t * d[0] - cb[0], ca[1] + t * d[1] - cb[1], ca[2] + t * d[2] - cb[2]];
        let q = [0, 1, 2].map(|i| rb[0][i] * p[0] + rb[1][i] * p[1] + rb[2][i] * p[2]);
        (f1 * q[0] / q[2] + w as f64 / 2.0, f1 * q[1] / q[2] + h as f64 / 2.0)
    };
    let mut messung = Vec::new();
    for an in [false, true] {
        let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: an };
        let e = zusammensetzen_intern(&bilder, &opt, &still).expect("Panorama");
        assert_eq!(e.verwendet.len(), 2, "{}", bericht_deutsch(&e.bericht));
        let n = &e.naht;
        let mut fehler = Vec::new();
        for y in 0..n.h {
            for x in 0..n.w - 1 {
                let (a, b) = (n.bild[y * n.w + x], n.bild[y * n.w + x + 1]);
                if a == u16::MAX || b == u16::MAX || a == b {
                    continue;
                }
                let (fx, fy) = ((x as f64 + 1.0) / n.massstab, (y as f64 + 0.5) / n.massstab);
                let (Some(pa), Some(pb)) = (e.leinwand_zu_bild(0, fx, fy), e.leinwand_zu_bild(1, fx, fy)) else { continue };
                if pa.0 < 0.0 || pa.1 < 0.0 || pa.0 > w as f64 || pa.1 > h as f64 {
                    continue;
                }
                let soll = wahr(pa.0, pa.1);
                fehler.push((soll.0 - pb.0).hypot(soll.1 - pb.1));
            }
        }
        fehler.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = fehler[fehler.len() / 2];
        eprintln!("Parallaxe {}: Versatz an der Naht Median {med:.2} px ({} Nahtpixel)\n{}", if an { "mit" } else { "ohne" }, fehler.len(), bericht_deutsch(&e.bericht));
        assert!(fehler.len() > 50, "zu wenig Nahtpixel: {}", fehler.len());
        messung.push((med, e.verformungen.iter().any(|v| v.is_some())));
    }
    // Gemessen: ohne 0,51 px, mit 0,16 px. Groessere Basis oder Schraege
    // machen die Abbildung der Wand so perspektivisch, dass Z&L Gl. 3 jede
    // Homographie verwirft — dann bleibt es (richtig) bei Teil A.
    assert!(messung[0].0 > 0.3, "Testaufbau ohne messbare Parallaxe: {}", messung[0].0);
    assert!(messung[1].1, "Ausgleich hat nichts verformt");
    assert!(messung[1].0 < 0.5 * messung[0].0, "ohne {:.2} px, mit {:.2} px", messung[0].0, messung[1].0);
}

/// Reine Drehung (keine Parallaxe): kein Paar ueber der Schwelle, nichts wird
/// verformt, die Ausgabe ist mit und ohne Ausgleich gleich.
#[test]
fn reine_drehung_bleibt_unverformt() {
    let lagen = [(-15.0, 0.0), (5.0, 0.0)];
    let fall = drehfall(71, &lagen, &[1.0, 1.0], &[0, 1]);
    let mut ausgaben = Vec::new();
    for an in [false, true] {
        let opt = Optionen { projektion: Projektion::Zylinder, belichtung_angleichen: true, max_ausgabe_seite: 30000, parallaxe_ausgleichen: an };
        let e = zusammensetzen_intern(&fall.bilder, &opt, &still).expect("Panorama");
        assert!(e.verformungen.iter().all(|v| v.is_none()), "{}", bericht_deutsch(&e.bericht));
        ausgaben.push(e.rgb);
    }
    assert_eq!(ausgaben[0], ausgaben[1]);
}
