//! Die Entwicklungs-Parameter — **schluesselgleich zur Web-Version**.
//!
//! Das ist Absicht und kein Zufall: Die KI antwortet mit genau diesen Namen
//! (`exposure`, `gradeShadowHue`, `hslS` …). Behalten wir sie bei, funktioniert der
//! bestehende KI-Vertrag unveraendert weiter, und gespeicherte Bearbeitungen
//! aus der Web-App lassen sich spaeter einlesen, statt sie wegzuwerfen.
//!
//! Einheiten sind ebenfalls die der Web-Version (meist -100..100). Was die Zahlen
//! physikalisch bedeuten, entscheidet erst der Shader.

use serde::{Deserialize, Serialize};

/// Acht Farbbereiche fuer HSL — Reihenfolge wie in der Web-Version.
pub const HUE_NAMES: [&str; 8] = [
    "Rot", "Orange", "Gelb", "Grün", "Türkis", "Blau", "Violett", "Magenta",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Edit {
    // --- Licht ---------------------------------------------------------------
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,

    // --- Farbe ---------------------------------------------------------------
    pub temp: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,

    // --- Farbverlauf (Split-Toning) ------------------------------------------
    #[serde(rename = "gradeShadowHue")]
    pub grade_shadow_hue: f32,
    #[serde(rename = "gradeShadowSat")]
    pub grade_shadow_sat: f32,
    #[serde(rename = "gradeMidHue")]
    pub grade_mid_hue: f32,
    #[serde(rename = "gradeMidSat")]
    pub grade_mid_sat: f32,
    #[serde(rename = "gradeHighHue")]
    pub grade_high_hue: f32,
    #[serde(rename = "gradeHighSat")]
    pub grade_high_sat: f32,
    #[serde(rename = "gradeBalance")]
    pub grade_balance: f32,

    // --- Effekte -------------------------------------------------------------
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub vignette: f32,
    pub grain: f32,

    // --- Details -------------------------------------------------------------
    pub sharpen: f32,
    #[serde(rename = "sharpenRadius")]
    pub sharpen_radius: f32,
    #[serde(rename = "sharpenMask")]
    pub sharpen_mask: f32,
    pub noise: f32,
    #[serde(rename = "colorNoise")]
    pub color_noise: f32,

    // --- Objektiv ------------------------------------------------------------
    #[serde(rename = "lensDistort")]
    pub lens_distort: f32,
    #[serde(rename = "lensDevignette")]
    pub lens_devignette: f32,
    /// Profilkorrektur (Verzeichnung aus dem Objektivprofil + gemessene
    /// Randabdunklung) — Standard AN, wie in der Referenz-Entwicklung bei RAF-Kameras.
    /// Aeltere Begleitdateien ohne das Feld bekommen ueber `default` ebenfalls
    /// AN. Kein KI-Schluessel: die KI beurteilt Bildinhalt, nicht Geometrie.
    #[serde(rename = "lensAuto", default = "wahr")]
    pub lens_auto: bool,
    /// Welches Objektivprofil wirkt, wenn die Profilkorrektur an ist:
    /// "" = automatisch (DNG-Daten → Profil → Schätzung), sonst eine feste
    /// Wahl aus dem Menü (siehe `objektiv::Objektiv::gewaehlt`):
    /// "b061", "iphone_haupt", "iphone_uw", "iphone_tele", "schaetzung" (Kennung: gespeicherte Schluessel).
    /// Kein KI-Schlüssel — die Wahl gehört dem Nutzer.
    #[serde(rename = "lensProfil", default)]
    pub lens_profil: String,
    pub blackpoint: f32,

    // --- Sonstiges -----------------------------------------------------------
    pub bw: bool,
    pub rotate: i32,
    #[serde(rename = "flipH")]
    pub flip_h: bool,
    #[serde(rename = "flipV")]
    pub flip_v: bool,
    /// Feinwinkel zum Geraderichten in Grad (Horizont). Getrennt von `rotate`,
    /// weil `rotate` verlustfrei ist und dieser Wert neu abtasten muss.
    pub straighten: f32,
    /// Stellen, die weggerechnet werden sollen: [x, y, radius] als Anteil der
    /// Bildbreite. Umkehrbar wie jeder andere Regler — das Original bleibt heil.
    #[serde(default)]
    pub heal: Vec<[f32; 3]>,
    #[serde(rename = "hslH")]
    pub hsl_h: [f32; 8],
    #[serde(rename = "hslS")]
    pub hsl_s: [f32; 8],
    #[serde(rename = "hslL")]
    pub hsl_l: [f32; 8],
    /// Tonwertkurve: y-Werte an x = 0, 0.25, 0.5, 0.75, 1.
    pub curve: [f32; 5],
    /// Grundhelligkeit in Blendenstufen, beim Oeffnen einmal gemessen.
    ///
    /// Bewusst **nicht** im Belichtungsregler: ein lineares RAW braucht rund
    /// +2 Blenden, damit es ueberhaupt normal aussieht. Steckte das im Regler,
    /// staende der beim Oeffnen schon am Anschlag und man haette keinen
    /// Spielraum mehr nach oben. So bleibt der Regler bei 0 und regelt ±2
    /// Blenden **um den richtigen Startpunkt herum**.
    /// Die KI setzt diesen Wert nie — sie kennt den Schluessel gar nicht.
    #[serde(rename = "baseEv")]
    pub base_ev: f32,
    /// Zuschnitt als Anteil des Bildes: [x, y, breite, hoehe] in 0..1.
    /// `None` = ganzes Bild. Bewusst als Anteil und nicht in Pixeln — so gilt
    /// derselbe Zuschnitt fuer Vorschau (verkleinert) und Export (volle Aufloesung).
    /// Und bewusst umkehrbar: das Original wird nie beschnitten.
    pub crop: Option<[f32; 4]>,
    /// Prozessversion: 1 = bis Fotowache 1.4 (globale Tonregler, eigener Look),
    /// 2 = ab 2.0 (Rechenweg der Referenz-Entwicklung). Begleitdateien ohne das Feld sind 1.
    #[serde(default = "prozess_alt")]
    pub prozess: u32,
    /// Hat jemand (Mensch oder KI) dieses Foto bearbeitet? Die Automatik setzt
    /// sichtbare Reglerwerte — die zaehlen nicht als Bearbeitung.
    #[serde(default)]
    pub bearbeitet: bool,
    /// Farbprofil: "" = automatisch (das installierte Standardprofil, wenn vorhanden),
    /// "fotoraw" = Fotowaches eigene Wiedergabe. Kein KI-Schluessel.
    #[serde(default)]
    pub profil: String,
}

/// Aktuelle Prozessversion.
pub const PROZESS: u32 = 2;

fn prozess_alt() -> u32 {
    1
}

fn wahr() -> bool {
    true
}

impl Default for Edit {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            temp: 0.0,
            tint: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            grade_shadow_hue: 0.0,
            grade_shadow_sat: 0.0,
            grade_mid_hue: 0.0,
            grade_mid_sat: 0.0,
            grade_high_hue: 0.0,
            grade_high_sat: 0.0,
            grade_balance: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            vignette: 0.0,
            grain: 0.0,
            sharpen: 0.0,
            sharpen_radius: 0.0,
            sharpen_mask: 0.0,
            noise: 0.0,
            color_noise: 0.0,
            lens_distort: 0.0,
            lens_devignette: 0.0,
            lens_auto: true,
            lens_profil: String::new(),
            blackpoint: 0.0,
            bw: false,
            rotate: 0,
            flip_h: false,
            flip_v: false,
            straighten: 0.0,
            heal: Vec::new(),
            hsl_h: [0.0; 8],
            hsl_s: [0.0; 8],
            hsl_l: [0.0; 8],
            curve: [0.0, 0.25, 0.5, 0.75, 1.0],
            base_ev: 0.0,
            crop: None,
            prozess: PROZESS,
            bearbeitet: false,
            profil: String::new(),
        }
    }
}

impl Edit {
    pub fn is_default(&self) -> bool {
        self == &Edit::default()
    }

    /// Uebernimmt nur die Felder, die in einer KI-Antwort tatsaechlich vorkamen.
    /// Fehlende Schluessel bleiben unveraendert — die KI muss nicht alles setzen.
    pub fn merge_json(&mut self, value: &serde_json::Value) -> Vec<String> {
        let mut applied = Vec::new();
        let Some(obj) = value.as_object() else {
            return applied;
        };
        // Umweg ueber die eigene Serialisierung: so bleibt die Schluesselzuordnung
        // an genau einer Stelle definiert (den `serde(rename)`-Angaben oben) und
        // kann nicht auseinanderlaufen.
        let Ok(serde_json::Value::Object(mut mine)) = serde_json::to_value(&*self) else {
            return applied;
        };
        for (k, v) in obj {
            // Die Objektivwahl gehoert dem Nutzer, nie der KI — sonst koennte
            // eine Antwort die Profilkorrektur still abschalten oder umstellen.
            // Ebenso Prozessversion, Bearbeitet-Kennzeichen und Farbprofil.
            if matches!(k.as_str(), "lensAuto" | "lensProfil" | "prozess" | "bearbeitet" | "profil") {
                continue;
            }
            if mine.contains_key(k) {
                mine.insert(k.clone(), v.clone());
                applied.push(k.clone());
            }
        }
        if let Ok(merged) = serde_json::from_value::<Edit>(serde_json::Value::Object(mine)) {
            *self = merged;
        }
        applied
    }
}

/// Werte, die der Entwicklungs-Schader in dieser Form erwartet (Layout wie
/// `Uniforms` in develop.wgsl). Die Umrechnung der Regler steht bewusst in
/// Rust und nicht im Schader: hier ist sie testbar.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EditUniform {
    pub kamera_zu_pro: [[f32; 4]; 3],
    pub belichtung: [f32; 4], // Gesamt-EV, Kontrast, Lichter, Tiefen
    pub ton2: [f32; 4],       // Weiss, Schwarz, Grauschleier (roh), Dunst
    pub lokal: [f32; 4],      // Klarheit, Profil-Schwarz, Randaufhellung, -
    pub farbe: [f32; 4],      // Dynamik, Saettigung, S/W, Koernung
    pub grade_lo: [f32; 4],
    pub grade_mi: [f32; 4],
    pub grade_hi: [f32; 4],
    pub vign: [f32; 4], // Vignette, Mittelpunkt, Koernungsgroesse, Texturpixel je Sensorpixel
    pub zuschnitt: [f32; 4],
    pub curve: [f32; 8],
    pub hsl_h: [f32; 8],
    pub hsl_s: [f32; 8],
    pub hsl_l: [f32; 8],
    pub lage: [f32; 4],
    pub tabellen: [u32; 4],
    pub hsm_dims: [u32; 4],
    pub look_dims: [u32; 4],
    pub basis_dims: [u32; 4],
    pub kal: [f32; 24],
    pub saat: [f32; 4],
    /// Bildstatistik der Basis (log2, ohne Belichtung): p2, p50, p98, p99,5.
    pub bild: [f32; 4],
    /// Geometrie: Feinwinkel (rad), manuelle Entzerrung k, Randmassstab, aktiv.
    pub geo: [f32; 4],
    /// Objektivprofil (ptlens a, b, c) + Fuellmassstab.
    pub lens1: [f32; 4],
    /// Radiale Kamera-Korrektur: Anzahl Stuetzstellen, Fuellmassstab,
    /// Farbsaum an (0/1), Vignette an (0/1).
    pub radial: [f32; 4],
    pub r_quelle: [f32; 16],
    pub r_ziel: [f32; 16],
    pub r_m: [f32; 16],
    pub r_ca_r: [f32; 16],
    pub r_ca_b: [f32; 16],
    pub r_gain: [f32; 16],
}

/// Was ausser den Reglern in den Uniform-Block muss — vom Bild und seinem
/// Profil bestimmt (siehe `GpuImage::dev_uniform`).
#[derive(Debug, Clone)]
pub struct UniformKontext {
    pub kamera_zu_pro: [[f32; 3]; 3],
    /// Zusaetzliche Blenden: Kamera-Grundbelichtung + Profilversatz.
    pub zusatz_ev: f32,
    /// Schwarz-Abzug der Standardentwicklung (DNG-SDK: 0,005), 0 = aus.
    pub profil_schwarz: f32,
    /// Objektiv-Randaufhellung (gemessen + Regler), 0..1.
    pub randaufhellung: f32,
    /// Kachel-Versatz und volle Bildgroesse in Pixeln dieser Textur.
    pub lage: [f32; 4],
    pub tabellen: [u32; 4],
    pub hsm_dims: [u32; 4],
    pub look_dims: [u32; 4],
    pub basis_dims: [u32; 4],
    pub kal: [f32; 24],
    pub bild: [f32; 4],
    /// Zuschnitt in Anteilen des SENSORBILDS (Drehung/Spiegelung herausgerechnet).
    pub zuschnitt_sensor: [f32; 4],
    /// Pixel dieser Textur je Sensorpixel (Vorschau < 1).
    pub textur_je_sensor: f32,
    pub geo: [f32; 4],
    pub lens1: [f32; 4],
    pub radial: Option<crate::objektiv::Radial>,
}

impl Edit {
    pub fn to_uniform(&self, k: &UniformKontext) -> EditUniform {
        let mut curve = [0f32; 8];
        curve[..5].copy_from_slice(&self.curve);
        let mut hsl_h = [0f32; 8];
        let mut hsl_s = [0f32; 8];
        let mut hsl_l = [0f32; 8];
        for i in 0..8 {
            hsl_h[i] = self.hsl_h[i] / 100.0;
            hsl_s[i] = self.hsl_s[i] / 100.0;
            hsl_l[i] = self.hsl_l[i] / 100.0;
        }
        let m = k.kamera_zu_pro;
        EditUniform {
            kamera_zu_pro: [
                [m[0][0], m[0][1], m[0][2], 0.0],
                [m[1][0], m[1][1], m[1][2], 0.0],
                [m[2][0], m[2][1], m[2][2], 0.0],
            ],
            // Belichtung: ±100 entspricht ±2 Blenden — exakt wie in der
            // Web-Version (`state.exposure/100 * 2.0`), damit KI-Werte gleich wirken.
            belichtung: [
                self.exposure / 100.0 * 2.0 + self.base_ev + k.zusatz_ev,
                self.contrast / 100.0,
                self.highlights / 100.0,
                self.shadows / 100.0,
            ],
            ton2: [
                self.whites / 100.0,
                self.blacks / 100.0,
                self.blackpoint,
                self.dehaze / 100.0,
            ],
            lokal: [self.clarity / 100.0, k.profil_schwarz, k.randaufhellung, 0.0],
            farbe: [
                self.vibrance / 100.0,
                self.saturation / 100.0,
                if self.bw { 1.0 } else { 0.0 },
                self.grain / 100.0,
            ],
            grade_lo: [
                self.grade_shadow_hue.to_radians(),
                self.grade_shadow_sat / 100.0,
                self.grade_balance / 100.0,
                0.0,
            ],
            grade_mi: [self.grade_mid_hue.to_radians(), self.grade_mid_sat / 100.0, 0.0, 0.0],
            grade_hi: [self.grade_high_hue.to_radians(), self.grade_high_sat / 100.0, 0.0, 0.0],
            // Koernung: ~1,6 Sensorpixel je Korn (Groesse 25 in der Referenz-Entwicklung).
            vign: [self.vignette / 100.0, 0.35, 1.6, k.textur_je_sensor],
            zuschnitt: k.zuschnitt_sensor,
            curve,
            hsl_h,
            hsl_s,
            hsl_l,
            lage: k.lage,
            tabellen: k.tabellen,
            hsm_dims: k.hsm_dims,
            look_dims: k.look_dims,
            basis_dims: k.basis_dims,
            kal: k.kal,
            saat: [0.37, 0.0, 0.0, 0.0],
            bild: k.bild,
            geo: k.geo,
            lens1: k.lens1,
            radial: match &k.radial {
                Some(r) => [r.n as f32, r.fuell, 1.0, 1.0],
                None => [0.0, 1.0, 0.0, 0.0],
            },
            r_quelle: k.radial.map(|r| r.r_quelle).unwrap_or([0.0; 16]),
            r_ziel: k.radial.map(|r| r.r_ziel).unwrap_or([0.0; 16]),
            r_m: k.radial.map(|r| r.m).unwrap_or([1.0; 16]),
            r_ca_r: k.radial.map(|r| r.ca_r).unwrap_or([0.0; 16]),
            r_ca_b: k.radial.map(|r| r.ca_b).unwrap_or([0.0; 16]),
            r_gain: k.radial.map(|r| r.gain).unwrap_or([1.0; 16]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ki_antwort_wird_uebernommen() {
        let mut e = Edit::default();
        let v: serde_json::Value = serde_json::from_str(
            r#"{"exposure": 12, "gradeShadowHue": 210, "hslS": [0,0,0,10,0,0,0,0], "quatsch": 5}"#,
        )
        .unwrap();
        let applied = e.merge_json(&v);
        assert_eq!(e.exposure, 12.0);
        assert_eq!(e.grade_shadow_hue, 210.0);
        assert_eq!(e.hsl_s[3], 10.0);
        // Unbekannte Schluessel werden ignoriert, nicht uebernommen:
        assert!(!applied.contains(&"quatsch".to_string()));
        // Nicht genannte Felder bleiben unveraendert:
        assert_eq!(e.contrast, 0.0);
    }

    #[test]
    fn belichtung_entspricht_der_webversion() {
        let e = Edit {
            exposure: 100.0,
            ..Default::default()
        };
        let k = UniformKontext {
            kamera_zu_pro: crate::color::IDENTITY,
            zusatz_ev: 0.0,
            profil_schwarz: 0.0,
            randaufhellung: 0.0,
            lage: [0.0, 0.0, 100.0, 100.0],
            tabellen: [0; 4],
            hsm_dims: [0; 4],
            look_dims: [0; 4],
            basis_dims: [1, 1, 0, 0],
            kal: [0.0; 24],
            bild: [-8.0, -3.0, -0.5, 0.0],
            zuschnitt_sensor: [0.0, 0.0, 1.0, 1.0],
            textur_je_sensor: 1.0,
            geo: [0.0; 4],
            lens1: [0.0, 0.0, 0.0, 1.0],
            radial: None,
        };
        let u = e.to_uniform(&k);
        assert!((u.belichtung[0] - 2.0).abs() < 1e-6, "100 muss +2 Blenden sein");
    }
}
