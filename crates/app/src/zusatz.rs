//! Hinzufuegen (App-Seite): eingesetzte Stuecke laden und neue erzeugen.
//!
//! Das erzeugte Bild liegt als PNG neben dem Foto
//! (`<foto>.fotowache-zusatz-<n>.png`), die Begleitdatei sagt, wo es hingehoert.
//! So bleibt es beim naechsten Oeffnen erhalten, ohne neu zu rechnen (eine
//! Erzeugung dauert rund anderthalb Minuten und faellt nie zweimal gleich aus).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pipeline::params::{Edit, Zusatz};
use pipeline::zusatz::Stueck;
use pipeline::{GpuImage, Retusche};

use crate::sprache::t;

static BILDER: Mutex<Option<HashMap<PathBuf, Arc<image::RgbImage>>>> = Mutex::new(None);

fn datei_pfad(raw: &Path, z: &Zusatz) -> PathBuf {
    raw.parent().unwrap_or(Path::new(".")).join(&z.datei)
}

/// Die eingesetzten Stuecke eines Fotos, fertig fuer die Pipeline. Fehlt eine
/// Datei, faellt das Stueck still weg (das Foto bleibt unveraendert).
pub fn stuecke(raw: &Path, zusaetze: &[Zusatz]) -> Vec<Stueck> {
    let mut g = BILDER.lock().unwrap_or_else(|e| e.into_inner());
    let speicher = g.get_or_insert_with(HashMap::new);
    zusaetze
        .iter()
        .filter_map(|z| {
            let p = datei_pfad(raw, z);
            let bild = match speicher.get(&p) {
                Some(b) => b.clone(),
                None => {
                    let b = Arc::new(image::open(&p).ok()?.to_rgb8());
                    speicher.insert(p, b.clone());
                    b
                }
            };
            Some(Stueck { kasten: z.kasten, weiss: z.weiss, flecken: z.flecken.clone(), bild })
        })
        .collect()
}

/// In die Ansichtslage drehen (wie `heal_punkt_drehen`: erst im Uhrzeigersinn
/// drehen, dann spiegeln) — der Generator soll den Himmel oben sehen.
fn in_ansicht(b: &image::RgbImage, e: &Edit) -> image::RgbImage {
    let mut b = match e.rotate.rem_euclid(360) {
        90 => image::imageops::rotate90(b),
        180 => image::imageops::rotate180(b),
        270 => image::imageops::rotate270(b),
        _ => b.clone(),
    };
    if e.flip_h {
        image::imageops::flip_horizontal_in_place(&mut b);
    }
    if e.flip_v {
        image::imageops::flip_vertical_in_place(&mut b);
    }
    b
}

/// Genau zurueck in die Lage der Rohdaten.
fn zurueck(b: &image::RgbImage, e: &Edit) -> image::RgbImage {
    let mut b = b.clone();
    if e.flip_v {
        image::imageops::flip_vertical_in_place(&mut b);
    }
    if e.flip_h {
        image::imageops::flip_horizontal_in_place(&mut b);
    }
    match e.rotate.rem_euclid(360) {
        90 => image::imageops::rotate270(&b),
        180 => image::imageops::rotate180(&b),
        270 => image::imageops::rotate90(&b),
        _ => b,
    }
}

/// Erzeugt ein neues Stueck: schneidet den markierten Bereich (mit Umgebung)
/// aus den Rohdaten, laesst den Generator den Text hineinsetzen und legt das
/// Ergebnis neben das Foto. Dauert rund anderthalb Minuten.
pub fn erzeugen(
    raw: &rawcore::RawImage,
    raw_pfad: &Path,
    edit: &Edit,
    flecken: Vec<[f32; 3]>,
    text: &str,
    seed: u64,
) -> Result<Zusatz, String> {
    if flecken.is_empty() {
        return Err(t("Erst den Bereich markieren, in den etwas hinzukommen soll.").into());
    }
    // Die bisherige Retusche gehoert zur Vorlage — sonst kaeme eine entfernte
    // Stelle im eingesetzten Bereich zurueck.
    let bisher = stuecke(raw_pfad, &edit.zusaetze);
    let retusche = Retusche { heal: &edit.heal, heal_ki: &edit.heal_ki, zusaetze: &bisher };
    let (w, h, data) = GpuImage::vorbereiten(raw, Some(4096), &retusche).map_err(|e| e.to_string())?;
    let kasten = pipeline::zusatz::kasten_um(&flecken, w as f32 / h as f32);
    let (vorlage, weiss) = pipeline::zusatz::ausschneiden(&data, w, h, kasten, 1024);
    let gedreht = in_ansicht(&vorlage, edit);
    let ergebnis = crate::generator::bearbeiten(&gedreht, text, seed)?;
    let mut ergebnis = zurueck(&ergebnis, edit);
    if ergebnis.dimensions() != vorlage.dimensions() {
        ergebnis = image::imageops::resize(&ergebnis, vorlage.width(), vorlage.height(), image::imageops::FilterType::Lanczos3);
    }
    // Farben an die Vorlage angleichen — gemessen im markierten Bereich.
    let (vw, vh) = (vorlage.width() as f32, vorlage.height() as f32);
    let [kx0, ky0, kx1, ky1] = kasten;
    let im_bereich = |x: u32, y: u32| {
        let fx = kx0 + (x as f32 + 0.5) / vw * (kx1 - kx0);
        let fy = ky0 + (y as f32 + 0.5) / vh * (ky1 - ky0);
        let seiten = w as f32 / h as f32;
        flecken.iter().any(|[a, b, r]| ((fx - a).powi(2) + ((fy - b) / seiten).powi(2)).sqrt() <= *r)
    };
    pipeline::zusatz::farben_angleichen(&mut ergebnis, &vorlage, &im_bereich);
    let name = raw_pfad.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let datei = format!("{name}.fotowache-zusatz-{seed:x}.png");
    let ziel = raw_pfad.parent().unwrap_or(Path::new(".")).join(&datei);
    ergebnis.save(&ziel).map_err(|e| e.to_string())?;
    Ok(Zusatz { datei, kasten, weiss, flecken, text: text.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drehen_und_zurueck_ist_verlustfrei() {
        let mut b = image::RgbImage::new(6, 4);
        for (x, y, p) in b.enumerate_pixels_mut() {
            *p = image::Rgb([x as u8, y as u8, (x * 7 + y) as u8]);
        }
        for rotate in [0, 90, 180, 270] {
            for (fh, fv) in [(false, false), (true, false), (false, true), (true, true)] {
                let e = Edit { rotate, flip_h: fh, flip_v: fv, ..Edit::default() };
                assert_eq!(zurueck(&in_ansicht(&b, &e), &e), b, "rotate {rotate} {fh} {fv}");
            }
        }
    }

    #[test]
    fn hochformat_steht_in_der_ansicht_aufrecht() {
        let b = image::RgbImage::new(6, 4);
        let e = Edit { rotate: 90, ..Edit::default() };
        assert_eq!(in_ansicht(&b, &e).dimensions(), (4, 6));
    }
}
