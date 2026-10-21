//! Panorama erstellen (App-Seite): Fotos laden, zusammensetzen lassen
//! (`pipeline::panorama`), Raender fuellen oder Zuschnitt vorschlagen und das
//! Ergebnis als lineare DNG neben die Fotos legen.
//!
//! Das Panorama ist danach eine Rohdatei wie jede andere: es erscheint in der
//! Galerie, wird mit allen Reglern entwickelt, die KI kann es entwickeln und
//! retuschieren, und der Zuschnitt ist das normale Werkzeug (R).

use std::path::{Path, PathBuf};

use pipeline::panorama::{Eingabe, Optionen, Projektion};

use crate::sprache::{self, t, tf};

/// Was erstellt werden soll.
#[derive(Debug, Clone)]
pub struct Auftrag {
    pub dateien: Vec<PathBuf>,
    pub projektion: Projektion,
    pub belichtung_angleichen: bool,
    /// Versatz naher Gegenstaende an den Naehten ausgleichen (Kamera nicht
    /// genau um ihr Projektionszentrum gedreht).
    pub parallaxe_ausgleichen: bool,
    /// Leere Raender mit dem Retusche-Modell fuellen (statt zuzuschneiden).
    pub raender_fuellen: bool,
    /// Ohne Fuellung: den groessten vollen Ausschnitt als Zuschnitt setzen.
    pub zuschneiden: bool,
    /// Laengste Seite je Eingabebild (Speicher/Tempo); 0 = volle Aufloesung.
    pub max_seite: u32,
}

/// Kameralage -> Drehung im Uhrzeigersinn (wie `drehung_aus_kamera`).
fn drehung(flip: i32) -> i32 {
    match flip {
        3 => 180,
        5 => 270,
        6 => 90,
        _ => 0,
    }
}

/// Dreht einen RGB-f32-Puffer im Uhrzeigersinn um 90/180/270 Grad.
fn drehen(w: u32, h: u32, rgb: Vec<f32>, grad: i32) -> (u32, u32, Vec<f32>) {
    let (w, h) = (w as usize, h as usize);
    match grad.rem_euclid(360) {
        90 | 270 => {
            let mut aus = vec![0f32; rgb.len()];
            for y in 0..h {
                for x in 0..w {
                    // 90: (x, y) -> (h-1-y, x); 270: (x, y) -> (y, w-1-x)
                    let (nx, ny) = if grad.rem_euclid(360) == 90 { (h - 1 - y, x) } else { (y, w - 1 - x) };
                    let (q, z) = ((y * w + x) * 3, (ny * h + nx) * 3);
                    aus[z..z + 3].copy_from_slice(&rgb[q..q + 3]);
                }
            }
            (h as u32, w as u32, aus)
        }
        180 => {
            let mut aus = rgb;
            let n = w * h;
            for i in 0..n / 2 {
                for c in 0..3 {
                    aus.swap(i * 3 + c, (n - 1 - i) * 3 + c);
                }
            }
            (w as u32, h as u32, aus)
        }
        _ => (w as u32, h as u32, rgb),
    }
}

/// Ein Foto als lineare Sensorwerte (0..1 = Saettigung), aufgerichtet und
/// auf `max_seite` verkleinert (Flaechenmittel, auch fuer krumme Faktoren —
/// ein ganzzahliger Schritt halbierte ein 6240-px-Foto bei Grenze 6000 auf 3120).
fn laden(pfad: &Path, max_seite: u32) -> Result<(Eingabe, rawcore::RawMeta), String> {
    let raw = rawcore::decode(pfad, &rawcore::DecodeOptions::default()).map_err(|e| format!("{}: {e}", pfad.display()))?;
    let (sw, sh) = (raw.width as usize, raw.height as usize);
    let faktor = if max_seite > 0 && sw.max(sh) > max_seite as usize { max_seite as f32 / sw.max(sh) as f32 } else { 1.0 };
    let (w, h) = (((sw as f32 * faktor).round() as usize).max(1), ((sh as f32 * faktor).round() as usize).max(1));
    // Weissabgleich der Datei herausrechnen: alle Bilder einer Reihe sollen
    // dieselben Sensorwerte zeigen, egal welchen Abgleich die Kamera je Bild
    // geschaetzt hat.
    let wb = raw.meta.wb_mul;
    let skala = [0, 1, 2].map(|c| 1.0 / 65535.0 / wb[c].max(1e-6));
    let mut rgb = vec![0f32; w * h * 3];
    if faktor >= 1.0 {
        for (i, v) in raw.pixels.iter().enumerate() {
            rgb[i] = *v as f32 * skala[i % 3];
        }
    } else {
        // Flaechenmittel: jedes Zielpixel mittelt die Quellpixel, die es
        // ueberdeckt (mit Anteilen an den Raendern).
        let sx = sw as f32 / w as f32;
        let sy = sh as f32 / h as f32;
        for y in 0..h {
            let (y0, y1) = (y as f32 * sy, (y + 1) as f32 * sy);
            for x in 0..w {
                let (x0, x1) = (x as f32 * sx, (x + 1) as f32 * sx);
                let mut s = [0f32; 3];
                let mut gewicht = 0f32;
                for yy in y0.floor() as usize..(y1.ceil() as usize).min(sh) {
                    let gy = (y1.min(yy as f32 + 1.0) - y0.max(yy as f32)).max(0.0);
                    for xx in x0.floor() as usize..(x1.ceil() as usize).min(sw) {
                        let g = gy * (x1.min(xx as f32 + 1.0) - x0.max(xx as f32)).max(0.0);
                        let i = (yy * sw + xx) * 3;
                        for c in 0..3 {
                            s[c] += raw.pixels[i + c] as f32 * g;
                        }
                        gewicht += g;
                    }
                }
                for c in 0..3 {
                    rgb[(y * w + x) * 3 + c] = s[c] / gewicht.max(1e-6) * skala[c];
                }
            }
        }
    }
    // Objektivkorrektur VOR dem Zusammensetzen (Brown & Lowe, „Advanced Camera
    // Modelling": die radiale Verzeichnung gehoert in Ausgleich und Darstellung —
    // hier ist sie mit dem Profil der Datei bzw. Datenbank vorher exakt
    // herausgerechnet). Randabdunklung gleich mit, sonst zeichnet sie jede Naht.
    let objektiv = pipeline::objektiv::erkennen(&raw);
    let rgb = entzerren(&rgb, w, h, &objektiv);
    let (w, h, rgb) = drehen(w as u32, h as u32, rgb, drehung(raw.meta.flip));
    // Brennweite in Pixeln: Kleinbild-Brennweite / 36 mm * laengste Seite, auf
    // das entzerrte Bild umgerechnet.
    let brennweite_px = (raw.meta.brennweite_kb > 0.0)
        .then(|| raw.meta.brennweite_kb / 36.0 * w.max(h) as f32 * objektiv.mitten_vergroesserung());
    // Entzerrt ist das Bild nur, wenn es ein Profil gab; sonst schaetzt der Kern
    // die Verzeichnung selbst (gemeinsam je Optik).
    let entzerrt = objektiv.abc != [0.0; 3] || objektiv.radial.is_some();
    Ok((Eingabe { w, h, rgb, brennweite_px, entzerrt }, raw.meta))
}

/// Objektivkorrektur auf einem linearen RGB-Puffer (Sensorlage): Verzeichnung
/// und Farbsaeume je Kanal (bilinear abgetastet), Randabdunklung als Faktor.
/// Ohne Profil kommt der Puffer unveraendert zurueck.
fn entzerren(rgb: &[f32], w: usize, h: usize, o: &pipeline::objektiv::Objektiv) -> Vec<f32> {
    if o.abc == [0.0; 3] && o.radial.is_none() {
        return rgb.to_vec();
    }
    let (bw, bh) = (w as f32, h as f32);
    let abtasten = |q: [f32; 2], c: usize| -> f32 {
        let x = (q[0] - 0.5).clamp(0.0, bw - 1.0);
        let y = (q[1] - 0.5).clamp(0.0, bh - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| rgb[(yy * w + xx) * 3 + c];
        (p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx) * (1.0 - fy) + (p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx) * fy
    };
    let mut aus = vec![0f32; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            let q = o.quelle([x as f32 + 0.5, y as f32 + 0.5], bw, bh);
            let gain = o.rand_gain(q[1], bw, bh);
            let i = (y * w + x) * 3;
            for c in 0..3 {
                aus[i + c] = abtasten(q[c], c) * gain;
            }
        }
    }
    aus
}

/// Freier Dateiname `Panorama-<erstes Foto>[-n].dng` neben dem ersten Foto.
fn zielpfad(erstes: &Path) -> PathBuf {
    let ordner = erstes.parent().unwrap_or(Path::new("."));
    let stamm = erstes.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let mut n = 0;
    loop {
        let name = if n == 0 { format!("Panorama-{stamm}.dng") } else { format!("Panorama-{stamm}-{n}.dng") };
        let p = ordner.join(name);
        if !p.exists() {
            return p;
        }
        n += 1;
    }
}

/// Erstellt das Panorama und gibt den Pfad der DNG zurueck.
pub fn erstellen(a: &Auftrag, fortschritt: &dyn Fn(f32, String)) -> Result<PathBuf, String> {
    if a.dateien.len() < 2 {
        return Err(t("Für ein Panorama mindestens zwei Fotos auswählen.").into());
    }
    let n = a.dateien.len() as f32;
    let mut bilder = Vec::with_capacity(a.dateien.len());
    let mut erste_meta = None;
    for (i, d) in a.dateien.iter().enumerate() {
        fortschritt(0.25 * i as f32 / n, tf("Lade Foto {} von {}", &[&(i + 1), &a.dateien.len()]));
        let (e, meta) = laden(d, a.max_seite)?;
        if erste_meta.is_none() {
            erste_meta = Some(meta);
        }
        bilder.push(e);
    }
    let meta = erste_meta.expect("mindestens zwei Fotos");
    let opt = Optionen {
        projektion: a.projektion,
        belichtung_angleichen: a.belichtung_angleichen,
        parallaxe_ausgleichen: a.parallaxe_ausgleichen,
        max_ausgabe_seite: 30000,
    };
    let mut erg = pipeline::panorama::zusammensetzen(&bilder, &opt, &|f, s| fortschritt(0.25 + 0.55 * f, sprache::panorama_schritt(s)))
        .map_err(|e| sprache::panorama_fehler(&e))?;
    drop(bilder);
    let mut zuschnitt = None;
    if a.raender_fuellen && pipeline::heal_ki::verfuegbar() {
        fortschritt(0.8, t("KI füllt die Ränder…").into());
        let mut leer: Vec<u8> = erg.deckung.iter().map(|d| (*d < 128) as u8).collect();
        // Das Panorama liegt als Sensorwerte OHNE Weissabgleich vor (gruenlastig).
        // Das Netz kennt nur abgeglichene Bilder und ergaenzt „neutrale" Flaechen,
        // die nach dem Abgleich beim Entwickeln lila wurden (gemessen) — also fuer
        // die Fuellung abgleichen und danach exakt zurueckrechnen.
        let wb = meta.wb_aufnahme;
        let g = wb[1].max(1e-6);
        let abgleich = [wb[0] / g, 1.0, wb[2] / g];
        for (i, v) in erg.rgb.iter_mut().enumerate() {
            *v *= abgleich[i % 3];
        }
        pipeline::heal_ki::flaechen_fuellen(&mut erg.rgb, erg.w, erg.h, &mut leer, &|f| fortschritt(0.8 + 0.15 * f, t("KI füllt die Ränder…").into()))?;
        for (i, v) in erg.rgb.iter_mut().enumerate() {
            *v /= abgleich[i % 3];
        }
    } else if a.zuschneiden {
        let [x0, y0, x1, y1] = pipeline::panorama::groesstes_rechteck(&erg.deckung, erg.w, erg.h);
        if x1 > x0 && y1 > y0 {
            let (w, h) = (erg.w as f32, erg.h as f32);
            zuschnitt = Some([x0 as f32 / w, y0 as f32 / h, (x1 - x0) as f32 / w, (y1 - y0) as f32 / h]);
        }
    }
    fortschritt(0.95, t("Panorama wird gespeichert…").into());
    let ziel = zielpfad(&a.dateien[0]);
    let wb = meta.wb_aufnahme;
    let herkunft = rawcore::dng_schreiben::Herkunft {
        make: meta.make.clone(),
        model: meta.model.clone(),
        cam_to_xyz: meta.cam_to_xyz,
        wb: [wb[0], wb[1], wb[2]],
        iso: meta.iso,
        shutter: meta.shutter,
        aperture: meta.aperture,
        focal_len: meta.focal_len,
        timestamp: meta.timestamp,
    };
    rawcore::dng_schreiben::schreiben(&ziel, erg.w, erg.h, &erg.rgb, &herkunft).map_err(|e| e.to_string())?;
    // Begleitdatei: Automatik fuer das Panorama + vorgeschlagener Zuschnitt.
    let raw = rawcore::decode(&ziel, &rawcore::DecodeOptions::default()).map_err(|e| e.to_string())?;
    let mut edit = crate::grundstand(&ziel, &raw);
    edit.crop = zuschnitt;
    edit.bearbeitet = true;
    crate::save_sidecar(&ziel, &edit, 0);
    fortschritt(1.0, sprache::panorama_bericht(&erg.bericht));
    Ok(ziel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drehen_um_90_grad_im_uhrzeigersinn() {
        // 2x1: links rot, rechts blau -> nach 90 Grad: oben rot, unten blau.
        let rgb = vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let (w, h, d) = drehen(2, 1, rgb, 90);
        assert_eq!((w, h), (1, 2));
        assert_eq!(&d[0..3], &[1.0, 0.0, 0.0]);
        assert_eq!(&d[3..6], &[0.0, 0.0, 1.0]);
        let (_, _, z) = drehen(w, h, d, 270);
        assert_eq!(z, vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn zielname_wird_nicht_ueberschrieben() {
        let d = std::env::temp_dir().join(format!("fotowache-pano-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let erstes = d.join("DSC1.RAF");
        let a = zielpfad(&erstes);
        std::fs::write(&a, b"x").unwrap();
        let b = zielpfad(&erstes);
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(d);
    }
}
