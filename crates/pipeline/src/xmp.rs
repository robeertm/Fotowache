//! XMP-Einstellungen aus Begleitdateien lesen.
//!
//! Gaengige Raw-Entwickler schreiben ihre Regler als `crs:`-Attribute in eine
//! `<name>.xmp` neben das RAW. Seit Prozess 2 haben Fotowaches Tonregler dieselbe
//! Bedeutung wie die der Referenz-Entwicklung (lokal, bildabhaengig) — eine
//! Bearbeitung von dort laesst sich deshalb direkt uebernehmen, statt sie
//! nachzubauen.
//!
//! Gelesen wird bewusst ohne XML-Bibliothek: die Werte stehen als einfache
//! Attribute `crs:Name="Wert"` (oder als `<crs:Name>Wert</crs:Name>`).

use crate::params::Edit;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Die gelesenen XMP-Einstellungen (nur, was Fotowache abbilden kann).
#[derive(Debug, Clone, Default)]
pub struct XmpEinstellungen {
    pub werte: HashMap<String, String>,
    /// Absoluter Weissabgleich, falls nicht „Wie Aufnahme".
    pub weissabgleich: Option<(f64, f64)>,
    pub bewertung: Option<u8>,
}

impl XmpEinstellungen {
    fn zahl(&self, schluessel: &str) -> Option<f32> {
        self.werte.get(schluessel).and_then(|v| v.trim().trim_start_matches('+').parse::<f32>().ok())
    }

    /// Stehen in der XMP ueberhaupt Entwicklungseinstellungen?
    pub fn hat_einstellungen(&self) -> bool {
        self.werte.contains_key("Exposure2012") || self.werte.contains_key("Highlights2012")
    }

    /// In Fotowaches Reglermodell uebersetzen. `wb_aufnahme` = (Kelvin, Toenung)
    /// der Aufnahme — die XMP speichert absolute Werte, Fotowache relative.
    pub fn als_edit(&self, basis: &Edit, wb_aufnahme: (f64, f64)) -> Edit {
        let mut e = basis.clone();
        let z = |k: &str| self.zahl(k);
        if let Some(v) = z("Exposure2012") {
            e.exposure = v * 50.0; // ±100 = ±2 Blenden
        }
        if let Some(v) = z("Contrast2012") {
            e.contrast = v;
        }
        if let Some(v) = z("Highlights2012") {
            e.highlights = v;
        }
        if let Some(v) = z("Shadows2012") {
            e.shadows = v;
        }
        if let Some(v) = z("Whites2012") {
            e.whites = v;
        }
        if let Some(v) = z("Blacks2012") {
            e.blacks = v;
        }
        if let Some(v) = z("Texture") {
            e.texture = v;
        }
        if let Some(v) = z("Clarity2012") {
            e.clarity = v;
        }
        if let Some(v) = z("Dehaze") {
            e.dehaze = v;
        }
        if let Some(v) = z("Vibrance") {
            e.vibrance = dynamik_aus_xmp(v);
        }
        if let Some(v) = z("Saturation") {
            e.saturation = v;
        }
        match self.weissabgleich {
            Some((k, t)) => {
                let (temp, tint) = crate::wb_regler(wb_aufnahme.0, wb_aufnahme.1, k, t);
                e.temp = temp;
                e.tint = tint;
            }
            None => {
                e.temp = 0.0;
                e.tint = 0.0;
            }
        }
        // Details: der Betrag 0..150 der XMP-Einstellungen entspricht unserem Regler direkt,
        // der Radius 1,0..3,0 unserem 0..100.
        if let Some(v) = z("Sharpness") {
            e.sharpen = v.clamp(0.0, 100.0);
        }
        if let Some(v) = z("SharpenRadius") {
            e.sharpen_radius = ((v - 1.0) / 2.0 * 100.0).clamp(0.0, 100.0);
        }
        if let Some(v) = z("SharpenEdgeMasking") {
            e.sharpen_mask = v.clamp(0.0, 100.0);
        }
        if let Some(v) = z("LuminanceSmoothing") {
            e.noise = v.clamp(0.0, 100.0);
        }
        if let Some(v) = z("ColorNoiseReduction") {
            e.color_noise = v.clamp(0.0, 100.0);
        }
        // HSL — gleiche Reihenfolge der acht Farbbereiche wie bei uns.
        for (i, name) in ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"]
            .iter()
            .enumerate()
        {
            if let Some(v) = z(&format!("HueAdjustment{name}")) {
                e.hsl_h[i] = v;
            }
            if let Some(v) = z(&format!("SaturationAdjustment{name}")) {
                e.hsl_s[i] = v;
            }
            if let Some(v) = z(&format!("LuminanceAdjustment{name}")) {
                e.hsl_l[i] = v;
            }
        }
        // Split-Toning / Farbverlauf (Naeherung).
        if let Some(v) = z("SplitToningShadowHue") {
            e.grade_shadow_hue = v;
        }
        if let Some(v) = z("SplitToningShadowSaturation") {
            e.grade_shadow_sat = v;
        }
        if let Some(v) = z("SplitToningHighlightHue") {
            e.grade_high_hue = v;
        }
        if let Some(v) = z("SplitToningHighlightSaturation") {
            e.grade_high_sat = v;
        }
        if let Some(v) = z("SplitToningBalance") {
            e.grade_balance = v;
        }
        if let Some(v) = z("ColorGradeMidtoneHue") {
            e.grade_mid_hue = v;
        }
        if let Some(v) = z("ColorGradeMidtoneSat") {
            e.grade_mid_sat = v;
        }
        // XMP-Einstellungen: negativer Betrag dunkelt ab — Fotowache: positiver.
        if let Some(v) = z("PostCropVignetteAmount") {
            e.vignette = -v;
        }
        if let Some(v) = z("GrainAmount") {
            e.grain = v;
        }
        if let Some(v) = z("LensProfileEnable") {
            e.lens_auto = v > 0.5;
        }
        if self.werte.get("ConvertToGrayscale").map(|v| v == "True").unwrap_or(false) {
            e.bw = true;
        }
        e
    }

    /// Zuschnitt aus den XMP-Einstellungen (Anteile, bezogen auf das Bild in Kameralage
    /// mit angewandter Ausrichtung) — `None`, wenn nicht zugeschnitten.
    pub fn zuschnitt(&self) -> Option<[f32; 4]> {
        if self.werte.get("HasCrop").map(|v| v != "True").unwrap_or(true) {
            return None;
        }
        let (t, l, b, r) = (
            self.zahl("CropTop")?,
            self.zahl("CropLeft")?,
            self.zahl("CropBottom")?,
            self.zahl("CropRight")?,
        );
        Some([l, t, (r - l).max(0.01), (b - t).max(0.01)])
    }
}

/// Liest die `crs:`-Werte einer XMP-Datei.
pub fn lesen_text(text: &str) -> XmpEinstellungen {
    let mut werte = HashMap::new();
    // Form 1: crs:Name="Wert"
    let mut rest = text;
    while let Some(p) = rest.find("crs:") {
        rest = &rest[p + 4..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let nach = &rest[name.len()..];
        if let Some(n) = nach.strip_prefix("=\"") {
            if let Some(ende) = n.find('"') {
                werte.entry(name).or_insert_with(|| n[..ende].to_string());
            }
        } else if let Some(n) = nach.strip_prefix('>') {
            // Form 2: <crs:Name>Wert</crs:Name> — nur einfache Werte.
            if let Some(ende) = n.find('<') {
                let wert = n[..ende].trim();
                if !wert.is_empty() {
                    werte.entry(name).or_insert_with(|| wert.to_string());
                }
            }
        }
    }
    let weissabgleich = match werte.get("WhiteBalance").map(|s| s.as_str()) {
        Some("As Shot") | None => None,
        _ => {
            let k = werte.get("Temperature").and_then(|v| v.trim_start_matches('+').parse::<f64>().ok());
            let t = werte.get("Tint").and_then(|v| v.trim_start_matches('+').parse::<f64>().ok());
            match (k, t) {
                (Some(k), Some(t)) => Some((k, t)),
                (Some(k), None) => Some((k, 0.0)),
                _ => None,
            }
        }
    };
    let bewertung = text
        .find("xmp:Rating=\"")
        .and_then(|p| text[p + 12..].chars().next())
        .and_then(|c| c.to_digit(10))
        .map(|d| d.min(5) as u8);
    XmpEinstellungen {
        werte,
        weissabgleich,
        bewertung,
    }
}

/// Wo liegt die XMP-Begleitdatei zu einem RAW? (`DSCF1400.xmp` oder `DSCF1400.RAF.xmp`)
pub fn pfad_zu(raw: &Path) -> Option<PathBuf> {
    let stamm = raw.with_extension("xmp");
    if stamm.is_file() {
        return Some(stamm);
    }
    let lang = PathBuf::from(format!("{}.xmp", raw.display()));
    lang.is_file().then_some(lang)
}

pub fn lesen(raw: &Path) -> Option<XmpEinstellungen> {
    let p = pfad_zu(raw)?;
    let text = std::fs::read_to_string(p).ok()?;
    let e = lesen_text(&text);
    e.hat_einstellungen().then_some(e)
}

/// Die Dynamik der XMP-Einstellungen in Fotowaches Dynamik umrechnen.
///
/// Bis +15 (wo die Automatik der Referenz-Entwicklung fast immer landet) wirken
/// beide gleich — gemessen an 38 Fotos, Farbkraft im Mittel 1,00. Darueber wirkt
/// Fotowaches Dynamik staerker: Porträts mit +40/+54 wurden 22–34 % farbiger als in
/// der Referenz-Entwicklung (Haut zu rot). Mit 40 % des Anteils ueber 15 trifft
/// die Farbkraft die Referenz (1,01/1,04), der Farbabstand sinkt von ΔE 4,7/6,8
/// auf 3,4/2,9 (Sonde `kalibrieren --dynamik-faktor`).
pub fn dynamik_aus_xmp(v: f32) -> f32 {
    if v > 15.0 {
        15.0 + (v - 15.0) * 0.4
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEISPIEL: &str = r#"<rdf:Description crs:Version="18.3" crs:ProcessVersion="15.4"
   crs:WhiteBalance="As Shot" crs:Exposure2012="+0.16" crs:Contrast2012="+6"
   crs:Highlights2012="-67" crs:Shadows2012="+38" crs:Whites2012="+16"
   crs:Blacks2012="-18" crs:Vibrance="+15" crs:Saturation="0" crs:Sharpness="40"
   crs:SharpenRadius="+1.0" crs:ColorNoiseReduction="25" crs:HasCrop="False"
   xmp:Rating="3">"#;

    #[test]
    fn xmp_werte_kommen_an() {
        let xmp = lesen_text(BEISPIEL);
        assert!(xmp.hat_einstellungen());
        assert_eq!(xmp.bewertung, Some(3));
        let e = xmp.als_edit(&Edit::default(), (5200.0, 5.0));
        assert!((e.exposure - 8.0).abs() < 1e-4, "0,16 EV = 8 Einheiten");
        assert_eq!(e.highlights, -67.0);
        assert_eq!(e.shadows, 38.0);
        assert_eq!(e.whites, 16.0);
        assert_eq!(e.blacks, -18.0);
        assert_eq!(e.vibrance, 15.0);
        // Ueber 15 abgeschwaecht (Fotowaches Dynamik wirkt dort staerker).
        assert_eq!(dynamik_aus_xmp(54.0), 15.0 + 39.0 * 0.4);
        assert_eq!(dynamik_aus_xmp(-20.0), -20.0);
        assert_eq!(e.sharpen, 40.0);
        assert_eq!(e.sharpen_radius, 0.0);
        assert_eq!(e.color_noise, 25.0);
        assert_eq!((e.temp, e.tint), (0.0, 0.0));
        assert!(xmp.zuschnitt().is_none());
    }

    #[test]
    fn eigener_weissabgleich_wird_relativ() {
        let xmp = lesen_text(r#"crs:WhiteBalance="Custom" crs:Temperature="4650" crs:Tint="+14" crs:Exposure2012="0""#);
        let e = xmp.als_edit(&Edit::default(), (5200.0, 4.0));
        // Kaelter als die Aufnahme → Temperatur-Regler negativ.
        assert!(e.temp < 0.0, "{}", e.temp);
        assert!((e.tint - 10.0).abs() < 1e-3);
    }
}
