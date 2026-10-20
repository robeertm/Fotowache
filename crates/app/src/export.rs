//! Export — einzeln und im Stapel.
//!
//! Wichtig: Exportiert wird **immer aus der vollen Aufloesung**, nie aus der
//! Bildschirmvorschau. Die Vorschau ist verkleinert, damit das Regeln fluessig
//! bleibt; wuerde man sie speichern, haenge die Qualitaet an der Fenstergroesse —
//! genau der Fehler, den die Web-Version hatte.

use crate::sprache::{t, tf};
use pipeline::{params::Edit, Gpu, GpuImage};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// JPEG, Qualitaet einstellbar — der Alltagsfall.
    Jpeg,
    /// PNG, 8 Bit verlustfrei — wenn nachbearbeitet wird.
    Png,
    /// TIFF, **echte 16 Bit** — volle Tiefe, fuer alles was noch weiterbearbeitet wird.
    Tiff16,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Tiff16 => "tif",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Jpeg => t("JPEG (klein, für alles)"),
            Format::Png => t("PNG (verlustfrei, 8 Bit)"),
            Format::Tiff16 => t("TIFF 16 Bit (volle Tiefe)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportSettings {
    pub format: Format,
    pub quality: u8,
    /// Laengste Kante; `None` = volle Aufloesung.
    pub max_dim: Option<u32>,
    pub target_dir: Option<PathBuf>,
    /// Anhang an den Dateinamen, damit nie ein Original ueberschrieben wird.
    /// Deutsch „_entwickelt" (wie seit jeher — vorhandene Exporte bekommen beim
    /// erneuten Entwickeln denselben Namen), Englisch „_developed": ein
    /// englischer Nutzer hatte nie Exporte mit deutschem Namen.
    pub suffix: String,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            format: Format::Jpeg,
            quality: 92,
            max_dim: None,
            target_dir: None,
            suffix: match crate::sprache::aktuell() {
                crate::sprache::Sprache::De => "_entwickelt".into(),
                crate::sprache::Sprache::En => "_developed".into(),
            },
        }
    }
}

impl ExportSettings {
    pub fn target_for(&self, src: &Path) -> PathBuf {
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "foto".into());
        let dir = self
            .target_dir
            .clone()
            .or_else(|| src.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        dir.join(format!("{stem}{}.{}", self.suffix, self.format.ext()))
    }
}

#[derive(Debug)]
pub enum ExportError {
    Raw(rawcore::RawError),
    Gpu(pipeline::GpuError),
    Write(String),
}

impl From<rawcore::RawError> for ExportError {
    fn from(e: rawcore::RawError) -> Self {
        ExportError::Raw(e)
    }
}

impl From<pipeline::GpuError> for ExportError {
    fn from(e: pipeline::GpuError) -> Self {
        ExportError::Gpu(e)
    }
}

/// Von Hand statt ueber `thiserror`: die Meldung entsteht erst beim Anzeigen,
/// damit sie in der gerade gewaehlten Sprache erscheint.
impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ExportError::Raw(e) => {
                tf("RAW konnte nicht gelesen werden: {}", &[&crate::sprache::raw_fehler(e)])
            }
            ExportError::Gpu(e) => tf("Grafikkarte: {}", &[&crate::sprache::gpu_fehler(e)]),
            ExportError::Write(e) => tf("Datei konnte nicht geschrieben werden: {}", &[e]),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExportError::Raw(e) => Some(e),
            ExportError::Gpu(e) => Some(e),
            ExportError::Write(_) => None,
        }
    }
}

/// Entwickelt eine Datei und schreibt das Ergebnis. Gibt den Zielpfad zurueck.
pub fn export_one(
    gpu: &Gpu,
    src: &Path,
    edit: &Edit,
    settings: &ExportSettings,
) -> Result<PathBuf, ExportError> {
    let raw = rawcore::decode(src, &rawcore::DecodeOptions::default())?;
    save_developed(gpu, &raw, src, edit, settings)
}

/// Erst ausrichten, dann zuschneiden.
///
/// Diese Reihenfolge ist keine Geschmacksfrage: zugeschnitten wird das Bild, das
/// man **sieht** — also das bereits gedrehte. Wuerde der Export zuerst schneiden,
/// saesse der Ausschnitt nach der Drehung an einer voellig anderen Stelle.
fn ausrichten_und_schneiden_rgba(
    rgba: &[u8],
    w: u32,
    h: u32,
    edit: &Edit,
) -> (Vec<u8>, u32, u32) {
    let (px, w, h) = pipeline::ausrichten(rgba, w, h, 4, edit.rotate, edit.flip_h, edit.flip_v);
    crop_rgba(&px, w, h, edit.crop)
}

fn ausrichten_und_schneiden_rgb16(
    px: &[u16],
    w: u32,
    h: u32,
    edit: &Edit,
) -> (Vec<u16>, u32, u32) {
    let (px, w, h) = pipeline::ausrichten(px, w, h, 3, edit.rotate, edit.flip_h, edit.flip_v);
    crop_rgb16(&px, w, h, edit.crop)
}

/// Entwickelt ein **bereits dekodiertes** RAW und schreibt es. Getrennt, damit der
/// KI-Stapel die Datei nur EINMAL dekodieren muss (40 MP ≈ 2 s — das doppelt zu
/// zahlen waere bei 200 Fotos eine Viertelstunde geschenkte Rechenzeit).
pub fn save_developed(
    gpu: &Gpu,
    raw: &rawcore::RawImage,
    src: &Path,
    edit: &Edit,
    settings: &ExportSettings,
) -> Result<PathBuf, ExportError> {
    // IMMER in voller Aufloesung entwickeln und erst danach verkleinern. Frueher
    // wurde VOR dem Entwickeln in ganzen Schritten verkleinert: aus „lange Kante
    // 2400" wurden 1938 px, und Schaerfe/Rauschminderung rechneten auf einem
    // anderen Bild als dem, das man in der Vorschau beurteilt hatte.
    let mut img = GpuImage::upload_mit(gpu, raw, None, &edit.heal)?;
    // Objektivkorrektur auch im Export — Vorschau und Datei muessen dasselbe
    // zeigen. `erkennen` misst zusaetzlich die Randabdunklung am Bild.
    img.objektiv = pipeline::objektiv::erkennen(raw);
    let target = settings.target_for(src);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ExportError::Write(e.to_string()))?;
    }

    // Aufnahmedaten und Farbprofil kommen in jede Datei — siehe `pipeline::exif`.
    let auf = pipeline::exif::Aufnahme::aus_meta(&raw.meta);
    let icc = pipeline::icc::srgb_profil();

    // 16-Bit-Weg getrennt: hier wird die 8-Bit-Stufe komplett uebersprungen,
    // die sonst jede Tonwertabstufung kostet.
    if settings.format == Format::Tiff16 {
        let rgb16 = img.render16(gpu, edit);
        let (rgb16, cw, ch) = ausrichten_und_schneiden_rgb16(&rgb16, img.width, img.height, edit);
        let (rgb16, cw, ch) = verkleinern16(rgb16, cw, ch, settings.max_dim);
        let datei = pipeline::exif::tiff16(&rgb16, cw, ch, &auf, &icc);
        std::fs::write(&target, &datei).map_err(|e| ExportError::Write(e.to_string()))?;
        return Ok(target);
    }

    let rgba = img.render(gpu, edit);

    // Zuschnitt zuletzt: die Regler haben auf dem ganzen Bild gerechnet, damit
    // Histogramm-abhaengige Schritte nicht davon abhaengen, wo gerade geschnitten ist.
    let (rgba, cw, ch) = ausrichten_und_schneiden_rgba(&rgba, img.width, img.height, edit);
    let buf = image::RgbaImage::from_raw(cw, ch, rgba)
        .ok_or_else(|| ExportError::Write(t("Puffergröße passt nicht").into()))?;
    let buf = verkleinern8(buf, settings.max_dim);
    let (cw, ch) = buf.dimensions();
    let dynimg = image::DynamicImage::ImageRgba8(buf);

    match settings.format {
        Format::Jpeg => {
            let rgb = dynimg.to_rgb8();
            let mut roh = Vec::new();
            let mut enc =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut roh, settings.quality);
            enc.encode_image(&rgb)
                .map_err(|e| ExportError::Write(e.to_string()))?;
            let exif = pipeline::exif::exif_tiff(&auf, cw, ch);
            let fertig = pipeline::exif::jpeg_mit_metadaten(&roh, &exif, &icc);
            std::fs::write(&target, &fertig).map_err(|e| ExportError::Write(e.to_string()))?;
        }
        Format::Png => {
            let mut roh = Vec::new();
            {
                let mut w = std::io::Cursor::new(&mut roh);
                dynimg
                    .to_rgb8()
                    .write_to(&mut w, image::ImageFormat::Png)
                    .map_err(|e| ExportError::Write(e.to_string()))?;
            }
            // PNG kennt kein EXIF-Segment im ueblichen Sinn, aber das Farbprofil
            // passt hinein — und das ist der Teil, der die Farben rettet.
            let fertig = pipeline::exif::png_mit_icc(&roh, &icc);
            std::fs::write(&target, &fertig).map_err(|e| ExportError::Write(e.to_string()))?;
        }
        Format::Tiff16 => unreachable!("oben getrennt behandelt"),
    }
    Ok(target)
}

/// Auf die gewuenschte lange Kante verkleinern (nie vergroessern), Lanczos,
/// danach eine leichte Ausgabeschaerfung wie „Bildschirm, Standard" in der
/// Referenz-Entwicklung:
/// Verkleinern macht weicher, die Schaerfung gibt die Kantenzeichnung zurueck.
fn verkleinern8(buf: image::RgbaImage, max_dim: Option<u32>) -> image::RgbaImage {
    let (w, h) = buf.dimensions();
    let Some((nw, nh)) = zielmass(w, h, max_dim) else {
        return buf;
    };
    let mut klein = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Lanczos3);
    ausgabe_schaerfen8(&mut klein, 0.35);
    klein
}

fn verkleinern16(px: Vec<u16>, w: u32, h: u32, max_dim: Option<u32>) -> (Vec<u16>, u32, u32) {
    let Some((nw, nh)) = zielmass(w, h, max_dim) else {
        return (px, w, h);
    };
    let Some(buf) = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(w, h, px) else {
        return (Vec::new(), 0, 0);
    };
    let klein = image::imageops::resize(&buf, nw, nh, image::imageops::FilterType::Lanczos3);
    (klein.into_raw(), nw, nh)
}

/// Zielgroesse fuer eine lange Kante — `None`, wenn nichts zu tun ist.
pub fn zielmass(w: u32, h: u32, max_dim: Option<u32>) -> Option<(u32, u32)> {
    let m = max_dim?;
    let lang = w.max(h);
    if m == 0 || lang <= m {
        return None;
    }
    let s = m as f64 / lang as f64;
    Some((((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1)))
}

/// Unscharfmaske mit kleinem Radius (3×3) fuer die verkleinerte Ausgabe.
fn ausgabe_schaerfen8(img: &mut image::RgbaImage, menge: f32) {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return;
    }
    let quelle = img.clone();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            for c in 0..3 {
                let m = quelle.get_pixel(x, y)[c] as f32;
                let nachbarn = (quelle.get_pixel(x - 1, y)[c] as f32
                    + quelle.get_pixel(x + 1, y)[c] as f32
                    + quelle.get_pixel(x, y - 1)[c] as f32
                    + quelle.get_pixel(x, y + 1)[c] as f32)
                    / 4.0;
                let v = m + (m - nachbarn) * menge;
                img.get_pixel_mut(x, y)[c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Schneidet einen RGBA-Puffer auf das Anteils-Rechteck zu.
pub fn crop_rgba(
    rgba: &[u8],
    w: u32,
    h: u32,
    crop: Option<[f32; 4]>,
) -> (Vec<u8>, u32, u32) {
    let Some([cx, cy, cw, chh]) = crop else {
        return (rgba.to_vec(), w, h);
    };
    let x0 = ((cx.clamp(0.0, 1.0) * w as f32) as u32).min(w.saturating_sub(1));
    let y0 = ((cy.clamp(0.0, 1.0) * h as f32) as u32).min(h.saturating_sub(1));
    let cw = ((cw.clamp(0.0, 1.0) * w as f32) as u32).clamp(1, w - x0);
    let ch = ((chh.clamp(0.0, 1.0) * h as f32) as u32).clamp(1, h - y0);
    let mut out = Vec::with_capacity((cw * ch * 4) as usize);
    for y in 0..ch {
        let s = (((y0 + y) * w + x0) * 4) as usize;
        out.extend_from_slice(&rgba[s..s + (cw * 4) as usize]);
    }
    (out, cw, ch)
}

/// Wie `crop_rgba`, aber fuer 16-Bit-RGB (drei Werte je Pixel).
pub fn crop_rgb16(px: &[u16], w: u32, h: u32, crop: Option<[f32; 4]>) -> (Vec<u16>, u32, u32) {
    let Some([cx, cy, cwf, chf]) = crop else {
        return (px.to_vec(), w, h);
    };
    let x0 = ((cx.clamp(0.0, 1.0) * w as f32) as u32).min(w.saturating_sub(1));
    let y0 = ((cy.clamp(0.0, 1.0) * h as f32) as u32).min(h.saturating_sub(1));
    let cw = ((cwf.clamp(0.0, 1.0) * w as f32) as u32).clamp(1, w - x0);
    let ch = ((chf.clamp(0.0, 1.0) * h as f32) as u32).clamp(1, h - y0);
    let mut out = Vec::with_capacity((cw * ch * 3) as usize);
    for y in 0..ch {
        let s = (((y0 + y) * w + x0) * 3) as usize;
        out.extend_from_slice(&px[s..s + (cw * 3) as usize]);
    }
    (out, cw, ch)
}

#[derive(Debug, Clone)]
pub enum BatchMsg {
    Started { total: usize },
    Done { file: PathBuf, target: PathBuf },
    Failed { file: PathBuf, error: String },
    Finished { ok: usize, failed: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// „Lange Kante 2400" heisst 2400 — nicht 1938 wie frueher (ganzzahlige
    /// Verkleinerung VOR dem Entwickeln).
    #[test]
    fn lange_kante_wird_exakt_getroffen() {
        assert_eq!(zielmass(7752, 5178, Some(2400)), Some((2400, 1603)));
        assert_eq!(zielmass(5178, 7752, Some(2400)), Some((1603, 2400)));
        assert_eq!(zielmass(1200, 800, Some(2400)), None, "nie vergroessern");
        assert_eq!(zielmass(1200, 800, None), None);
    }
}
