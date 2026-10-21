//! Lokale KI — das Foto verstehen, dann nach festen Regeln entwickeln.
//!
//! ## Arbeitsteilung
//! Ein kleines Bildmodell auf dem eigenen Rechner (`ki_lokal`) ist gut darin, ein
//! Foto zu VERSTEHEN: was ist zu sehen, welches Licht, Menschen, Himmel, Dunst,
//! ein ungewollter Farbstich. Zahlen fuer Regler waehlt es dagegen schlecht —
//! gemessen: es setzte fast alles auf 0 oder schrieb die Beispielwerte aus der
//! Anweisung ab. Deshalb:
//!
//! 1. **Beschreiben** — das Modell beschreibt das Foto frei (darin ist es gut).
//! 2. **Einordnen** — im selben Gespraech ordnet es seine Beschreibung in feste
//!    Kategorien ein. Ohne erzwungenes Antwortschema: mit Schema waehlte es
//!    gemessen fast immer die ersten Werte der Listen („Feier in Innenraeumen"
//!    fuer eine Berglandschaft). Gelesen wird nachsichtig (`Einordnung::lesen`).
//! 3. **Regeln** — Fotowache macht daraus Regler: ein Grundlook je Szene, ein
//!    Zuschlag je gewuenschtem Look, dazu Korrekturen aus GEMESSENEN Werten
//!    (Lichter-Beschnitt, Helligkeit, Farbstich) — jeweils nur, wenn Messung und
//!    Einschaetzung des Modells zusammenpassen.
//!
//! Alles liegt auf der Automatik obendrauf, und die Leitplanken der
//! Aufnahme-Grundeinstellungen gelten weiter.

use crate::sprache::{t, tf};
use pipeline::params::Edit;

/// Wie kräftig die KI entwickeln soll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Sanft,
    Normal,
    Kraeftig,
}

impl Strength {
    fn faktor(self) -> f32 {
        match self {
            Strength::Sanft => 0.6,
            Strength::Normal => 1.0,
            Strength::Kraeftig => 1.5,
        }
    }
    /// Fuer die Oberflaeche.
    pub fn label(self) -> &'static str {
        match self {
            Strength::Sanft => t("sanft"),
            Strength::Normal => t("normal"),
            Strength::Kraeftig => t("kräftig"),
        }
    }
}

/// Was die KI zurückgemeldet hat — Regler plus Beschreibung.
#[derive(Debug, Clone, Default)]
pub struct AiResult {
    pub edit: Edit,
    pub applied: Vec<String>,
    pub scene_label: String,
    pub look: String,
    pub description: String,
    /// Die Antworten des Modells im Wortlaut — fuers Protokoll.
    pub roh: String,
    /// Welche Werte sich tatsaechlich geaendert haben, als Text.
    pub geaendert: Vec<String>,
    /// Was gegen die Leitplanken verstiess und zurechtgerueckt wurde.
    pub zurechtgerueckt: Vec<String>,
}

#[derive(Debug)]
pub enum AiError {
    Dienst(String),
    Encode(String),
}

/// Von Hand statt ueber `thiserror`: die Meldung entsteht erst beim Anzeigen,
/// in der gerade gewaehlten Sprache.
impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            AiError::Dienst(e) => tf("Lokale KI: {}", &[e]),
            AiError::Encode(e) => tf("Bild konnte nicht verpackt werden: {}", &[e]),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for AiError {}

// ---------------------------------------------------------------------------
// Einordnung
// ---------------------------------------------------------------------------

/// Die Kategorien, in die das Modell das Foto einordnet. Die Werte sind der
/// Vertrag mit dem Modell (englisch, so steht es in der Anweisung).
pub const SZENEN: &[&str] = &[
    "portrait", "group_of_people", "event_or_party_indoors", "interior_room", "landscape",
    "city_or_architecture", "sunset_or_sunrise", "night", "food_or_still_life", "nature_closeup",
    "beach_or_snow", "aerial_view", "other",
];
const LICHT: &[&str] = &[
    "direct_sun", "overcast_or_shade", "golden_hour", "warm_indoor_light", "mixed_indoor_light", "flash",
    "backlight", "night_lights",
];
const MENSCHEN: &[&str] = &["none", "small_in_frame", "prominent_faces"];
const HIMMEL: &[&str] = &["none", "some", "large_part"];
const DUNST: &[&str] = &["no", "yes"];
const HELLIGKEIT: &[&str] = &["too_dark", "slightly_dark", "right", "slightly_bright", "too_bright"];
const STICH: &[&str] = &["none_or_intended", "too_warm_yellow", "too_cool_blue", "too_green", "too_magenta"];
pub const LOOKS: &[&str] = &["natural", "vivid", "soft", "dramatic", "warm", "cool"];

const FELDER: &[(&str, &[&str])] = &[
    ("scene", SZENEN),
    ("light", LICHT),
    ("people", MENSCHEN),
    ("sky", HIMMEL),
    ("haze", DUNST),
    ("brightness", HELLIGKEIT),
    ("colour_cast", STICH),
    ("look", LOOKS),
];

/// Erste Frage: frei beschreiben.
const FRAGE_BESCHREIBEN: &str = "Describe this photo for a photo editor in 2-3 sentences: the main subject, \
indoors or outdoors, people and faces, how much sky, the light, the colours, and anything that looks wrong \
(too dark, too bright, colour cast, haze).";

fn frage_einordnen() -> String {
    let mut s = String::from(
        "Based on your description, classify the photo. Answer with ONE JSON object and nothing else, \
         choosing exactly one allowed value per key:\n",
    );
    for (k, werte) in FELDER {
        s.push_str(&format!("\"{k}\": {}\n", werte.join(" | ")));
    }
    // Die Beschreibung in der Sprache der Oberflaeche.
    let sprache = match crate::sprache::aktuell() {
        crate::sprache::Sprache::De => "German",
        crate::sprache::Sprache::En => "English",
    };
    s.push_str(&format!(
        "\"description\": one short friendly {sprache} sentence about what is in the photo.\n\
         Rules: colour_cast only for an UNWANTED cast (warm sunset or candle light is intended). \
         haze only for real fog or haze. look = the finished look that would suit the photo best."
    ));
    s
}

/// Die gelesene Einordnung. Fehlt ein Feld, gilt der neutrale Wert.
#[derive(Debug, Clone, PartialEq)]
pub struct Einordnung {
    pub szene: &'static str,
    pub licht: &'static str,
    pub menschen: &'static str,
    pub himmel: &'static str,
    pub dunst: bool,
    pub helligkeit: &'static str,
    pub stich: &'static str,
    pub look: &'static str,
    pub beschreibung: String,
}

impl Einordnung {
    /// Liest die Antwort nachsichtig: je Feld der erste erlaubte Wert, der nach
    /// dem Feldnamen steht. Das Modell setzt gelegentlich `\"` oder Zeilen-
    /// umbrueche, wo JSON keine erlaubt — ein strenger Parser verwarf dann
    /// die ganze Antwort (gemessen: 18 von 28).
    pub fn lesen(roh: &str) -> Option<Einordnung> {
        let wert = |feld: &str, erlaubt: &[&'static str]| -> Option<&'static str> {
            let i = roh.find(&format!("\"{feld}"))? + feld.len() + 1;
            let rest = &roh[i..roh.len().min(i + 80)];
            // Den frühesten passenden Wert nehmen; laengere zuerst, damit
            // „slightly_dark" nicht als „dark"-Teil missverstanden wird.
            erlaubt
                .iter()
                .filter_map(|w| rest.find(w).map(|p| (p, std::cmp::Reverse(w.len()), *w)))
                .min()
                .map(|(_, _, w)| w)
        };
        let szene = wert("scene", SZENEN)?;
        // „description" (frueher „description_de" — beide lesen).
        let beschreibung = roh
            .find("\"description")
            .map(|i| &roh[i + 1..])
            .map(|r| r.trim_start_matches("description").trim_start_matches("_de"))
            .map(|r| {
                let r = r.trim_start_matches(|c: char| c == '"' || c == '\\' || c == ':' || c.is_whitespace());
                let ende = r.find(|c: char| c == '"' || c == '\\' || c == '\n').unwrap_or(r.len());
                r[..ende].trim().to_string()
            })
            .unwrap_or_default();
        Some(Einordnung {
            szene,
            licht: wert("light", LICHT).unwrap_or("overcast_or_shade"),
            menschen: wert("people", MENSCHEN).unwrap_or("none"),
            himmel: wert("sky", HIMMEL).unwrap_or("none"),
            dunst: wert("haze", DUNST) == Some("yes"),
            helligkeit: wert("brightness", HELLIGKEIT).unwrap_or("right"),
            stich: wert("colour_cast", STICH).unwrap_or("none_or_intended"),
            look: wert("look", LOOKS).unwrap_or("natural"),
            beschreibung,
        })
    }
}

/// Deutsche Anzeige der Szene (laeuft durch die Uebersetzung).
pub fn szene_text(s: &str) -> &'static str {
    match s {
        "portrait" => t("Porträt"),
        "group_of_people" => t("Menschen in der Gruppe"),
        "event_or_party_indoors" => t("Feier in Innenräumen"),
        "interior_room" => t("Innenraum"),
        "landscape" => t("Landschaft"),
        "city_or_architecture" => t("Stadt und Architektur"),
        "sunset_or_sunrise" => t("Sonnenauf- oder -untergang"),
        "night" => t("Nacht"),
        "food_or_still_life" => t("Essen oder Stillleben"),
        "nature_closeup" => t("Natur von nah"),
        "beach_or_snow" => t("Strand oder Schnee"),
        "aerial_view" => t("Luftaufnahme"),
        _ => t("Sonstiges"),
    }
}

/// Deutsche Anzeige des Looks.
pub fn look_text(s: &str) -> &'static str {
    match s {
        "vivid" => t("lebendig"),
        "soft" => t("weich"),
        "dramatic" => t("dramatisch"),
        "warm" => t("warm"),
        "cool" => t("kühl"),
        _ => t("natürlich"),
    }
}

// ---------------------------------------------------------------------------
// Messwerte
// ---------------------------------------------------------------------------

/// Am entwickelten Vorschaubild gemessen (sRGB, 8 Bit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Messwerte {
    /// Logarithmisches Mittel der Helligkeit in Blenden gegen Mittelgrau.
    pub helligkeit_ev: f32,
    /// Anteil ausgefressener Pixel in Prozent.
    pub lichter_prozent: f32,
    /// Neutralton: R - B und G - (R+B)/2 der fast farblosen Mitten (0..255).
    pub warm_kalt: f32,
    pub gruen_magenta: f32,
}

impl Messwerte {
    pub fn aus(rgba: &[u8], w: u32, h: u32) -> Messwerte {
        let lin = |v: u8| {
            let c = v as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        let schritt = ((w.max(h) / 512).max(1)) as usize;
        let (mut log_summe, mut n, mut hell) = (0.0f64, 0usize, 0usize);
        let (mut rb, mut gm, mut nn) = (0.0f64, 0.0f64, 0usize);
        for y in (0..h as usize).step_by(schritt) {
            for x in (0..w as usize).step_by(schritt) {
                let i = (y * w as usize + x) * 4;
                let (r, g, b) = (rgba[i], rgba[i + 1], rgba[i + 2]);
                let yl = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
                log_summe += ((yl + 1e-4) as f64).ln();
                n += 1;
                if r.max(g).max(b) > 251 {
                    hell += 1;
                }
                let sat = r.max(g).max(b) as i32 - r.min(g).min(b) as i32;
                if sat < 30 && yl > 0.05 && yl < 0.8 {
                    rb += r as f64 - b as f64;
                    gm += g as f64 - (r as f64 + b as f64) / 2.0;
                    nn += 1;
                }
            }
        }
        let n = n.max(1);
        let mittel = (log_summe / n as f64).exp() as f32;
        let (warm_kalt, gruen_magenta) = if nn > 200 {
            ((rb / nn as f64) as f32, (gm / nn as f64) as f32)
        } else {
            (0.0, 0.0)
        };
        Messwerte {
            helligkeit_ev: (mittel.max(1e-4) / 0.18).log2(),
            lichter_prozent: hell as f32 * 100.0 / n as f32,
            warm_kalt,
            gruen_magenta,
        }
    }
}

// ---------------------------------------------------------------------------
// Regeln
// ---------------------------------------------------------------------------

/// Aenderungen gegenueber der Automatik.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Zuschlag {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub temp: f32,
    pub tint: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub texture: f32,
}

impl Zuschlag {
    #[allow(clippy::too_many_arguments)]
    const fn neu(contrast: f32, highlights: f32, shadows: f32, whites: f32, blacks: f32, vibrance: f32, saturation: f32, clarity: f32, dehaze: f32, texture: f32) -> Zuschlag {
        Zuschlag { exposure: 0.0, contrast, highlights, shadows, whites, blacks, temp: 0.0, tint: 0.0, vibrance, saturation, clarity, dehaze, texture }
    }
    fn plus(mut self, o: Zuschlag) -> Zuschlag {
        self.exposure += o.exposure;
        self.contrast += o.contrast;
        self.highlights += o.highlights;
        self.shadows += o.shadows;
        self.whites += o.whites;
        self.blacks += o.blacks;
        self.temp += o.temp;
        self.tint += o.tint;
        self.vibrance += o.vibrance;
        self.saturation += o.saturation;
        self.clarity += o.clarity;
        self.dehaze += o.dehaze;
        self.texture += o.texture;
        self
    }
    fn mal(mut self, f: f32) -> Zuschlag {
        for v in [
            &mut self.exposure, &mut self.contrast, &mut self.highlights, &mut self.shadows, &mut self.whites,
            &mut self.blacks, &mut self.temp, &mut self.tint, &mut self.vibrance, &mut self.saturation,
            &mut self.clarity, &mut self.dehaze, &mut self.texture,
        ] {
            *v *= f;
        }
        self
    }
}

/// Grundlook je Szene: Kontrast, Lichter, Tiefen, Weiss, Schwarz, Dynamik,
/// Saettigung, Klarheit, Dunst entfernen, Struktur.
fn szene_zuschlag(szene: &str) -> Zuschlag {
    match szene {
        "portrait" => Zuschlag::neu(3.0, -6.0, 8.0, 3.0, -3.0, 3.0, 0.0, -6.0, 0.0, -8.0),
        "group_of_people" => Zuschlag::neu(4.0, -6.0, 8.0, 3.0, -3.0, 4.0, 0.0, 2.0, 0.0, 0.0),
        "event_or_party_indoors" => Zuschlag::neu(4.0, -6.0, 10.0, 5.0, -3.0, 3.0, 0.0, 3.0, 0.0, 0.0),
        "interior_room" => Zuschlag::neu(4.0, -8.0, 10.0, 5.0, -3.0, 3.0, 0.0, 4.0, 0.0, 2.0),
        "landscape" => Zuschlag::neu(6.0, -10.0, 6.0, 4.0, -5.0, 14.0, 3.0, 8.0, 5.0, 6.0),
        "city_or_architecture" => Zuschlag::neu(8.0, -8.0, 6.0, 4.0, -6.0, 6.0, 0.0, 10.0, 3.0, 8.0),
        "sunset_or_sunrise" => Zuschlag::neu(6.0, -12.0, 5.0, 0.0, -5.0, 12.0, 5.0, 4.0, 4.0, 0.0),
        "night" => Zuschlag::neu(5.0, -10.0, 3.0, 0.0, -8.0, 5.0, 0.0, 4.0, 0.0, 0.0),
        "food_or_still_life" => Zuschlag::neu(5.0, -6.0, 5.0, 4.0, -4.0, 10.0, 3.0, 4.0, 0.0, 8.0),
        "nature_closeup" => Zuschlag::neu(5.0, -6.0, 5.0, 3.0, -4.0, 12.0, 2.0, 4.0, 0.0, 10.0),
        "beach_or_snow" => Zuschlag::neu(3.0, -10.0, 3.0, 3.0, -4.0, 8.0, 0.0, 4.0, 3.0, 0.0),
        "aerial_view" => Zuschlag::neu(6.0, -6.0, 5.0, 3.0, -5.0, 10.0, 0.0, 8.0, 8.0, 6.0),
        _ => Zuschlag::neu(5.0, -10.0, 8.0, 4.0, -5.0, 8.0, 0.0, 4.0, 0.0, 0.0),
    }
}

fn look_zuschlag(look: &str) -> Zuschlag {
    let mut z = match look {
        "vivid" => Zuschlag::neu(4.0, 0.0, 0.0, 0.0, 0.0, 8.0, 4.0, 3.0, 0.0, 0.0),
        "soft" => Zuschlag::neu(-6.0, -5.0, 0.0, 0.0, 0.0, 0.0, 0.0, -6.0, 0.0, -4.0),
        "dramatic" => Zuschlag::neu(10.0, 0.0, 0.0, 0.0, -6.0, 4.0, 0.0, 8.0, 4.0, 0.0),
        _ => Zuschlag::default(),
    };
    match look {
        "warm" => z.temp = 3.0,
        "cool" => z.temp = -5.0,
        _ => {}
    }
    z
}

/// Die Regeln: Einordnung + Messung -> Zuschlag auf die Automatik.
pub fn zuschlag(e: &Einordnung, m: &Messwerte, staerke: Strength) -> Zuschlag {
    let f = staerke.faktor();
    let mut z = szene_zuschlag(e.szene).plus(look_zuschlag(e.look));
    if e.dunst {
        z = z.plus(Zuschlag { dehaze: 10.0, contrast: 4.0, ..Default::default() });
    }
    if e.himmel == "large_part" {
        z = z.plus(Zuschlag { highlights: -5.0, dehaze: 3.0, ..Default::default() });
    }
    if e.licht == "backlight" && e.menschen != "none" {
        z = z.plus(Zuschlag { shadows: 12.0, ..Default::default() });
    }
    z = z.mal(f);
    // Gesichter: keine harte Struktur, gleich bei welcher Staerke.
    if e.menschen == "prominent_faces" {
        z.clarity = z.clarity.min(0.0);
        z.texture = z.texture.min(0.0);
    }
    // Helligkeit: die Automatik ist an echten fertigen Entwicklungen gelernt und
    // trifft sie gut. Das Modell darf sie nur verschieben, wenn die Messung es
    // KLAR bestaetigt — gemessen hat es „zu hell" auch bei richtig belichteten
    // Fotos gesagt und sie damit abgedunkelt.
    let ev = m.helligkeit_ev;
    let belichtung: f32 = match e.helligkeit {
        "too_dark" if ev < -1.7 => 15.0,
        "too_bright" if ev > -0.4 => -15.0,
        _ => 0.0,
    };
    z.exposure += if e.szene == "night" { belichtung.min(5.0) } else { belichtung } * f.min(1.2);
    // Ausgefressene Lichter: gemessen, nicht geschaetzt.
    if m.lichter_prozent > 1.0 {
        z.highlights -= (m.lichter_prozent * 3.0).min(10.0);
    }
    // Der ganze Lichter-Zuschlag bleibt maessig: mehrere Regeln addierten sich
    // gemessen auf -45, das Foto wurde flau und dunkel.
    z.highlights = z.highlights.max(-15.0 * f.max(1.0));
    // Farbstich nur, wenn Modell und Messung dieselbe Richtung sehen. Halbe
    // Korrektur: ~0,36 R-B-Punkte je Regler-Einheit (Kalibrierung der Grundwaerme).
    let licht_gewollt_warm = matches!(e.licht, "golden_hour" | "warm_indoor_light") || e.szene == "sunset_or_sunrise";
    match e.stich {
        "too_warm_yellow" if m.warm_kalt > 4.0 && !licht_gewollt_warm => {
            z.temp -= (m.warm_kalt / 0.36 * 0.5).clamp(4.0, 15.0);
        }
        "too_cool_blue" if m.warm_kalt < -4.0 => {
            z.temp += (-m.warm_kalt / 0.36 * 0.5).clamp(4.0, 15.0);
        }
        "too_green" if m.gruen_magenta > 3.0 => z.tint += (m.gruen_magenta * 1.5).clamp(3.0, 10.0),
        "too_magenta" if m.gruen_magenta < -3.0 => z.tint -= (-m.gruen_magenta * 1.5).clamp(3.0, 10.0),
        _ => {}
    }
    z
}

fn anwenden(basis: &Edit, z: &Zuschlag) -> Edit {
    let mut e = basis.clone();
    let k = |v: f32| v.clamp(-100.0, 100.0);
    e.exposure = k(e.exposure + z.exposure);
    e.contrast = k(e.contrast + z.contrast);
    e.highlights = k(e.highlights + z.highlights);
    e.shadows = k(e.shadows + z.shadows);
    e.whites = k(e.whites + z.whites);
    e.blacks = k(e.blacks + z.blacks);
    e.temp = k(e.temp + z.temp);
    e.tint = k(e.tint + z.tint);
    e.vibrance = k(e.vibrance + z.vibrance);
    e.saturation = k(e.saturation + z.saturation);
    e.clarity = k(e.clarity + z.clarity);
    e.dehaze = k(e.dehaze + z.dehaze);
    e.texture = k(e.texture + z.texture);
    e
}

/// Rechnet einen Bearbeitungsstand klein durch (RGBA, Breite, Hoehe) — zum
/// Nachmessen der Helligkeit.
pub type Probe<'a> = &'a dyn Fn(&Edit) -> Option<(Vec<u8>, u32, u32)>;

/// Fragt die lokale KI und gibt die entwickelten Regler zurück.
///
/// `base_edit` ist der aktuelle Stand (in der Regel die Automatik) — die KI legt
/// ihren Look obendrauf. Mit `probe` wird das Ergebnis nachgemessen (siehe
/// `nachmessen`).
pub fn develop(rgba: &[u8], width: u32, height: u32, base_edit: &Edit, strength: Strength, probe: Option<Probe>) -> Result<AiResult, AiError> {
    let jpeg = encode_small_jpeg(rgba, width, height, 896)?;
    let mess = Messwerte::aus(rgba, width, height);
    let beschreibung = crate::ki_lokal::fragen(&jpeg, &[], FRAGE_BESCHREIBEN, 160).map_err(AiError::Dienst)?;
    let frage2 = frage_einordnen();
    let roh = crate::ki_lokal::fragen(&jpeg, &[(FRAGE_BESCHREIBEN, &beschreibung)], &frage2, 240)
        .map_err(AiError::Dienst)?;
    let einordnung = Einordnung::lesen(&roh).ok_or_else(|| AiError::Dienst(t("Antwort ließ sich nicht einordnen").into()))?;
    let mut r = develop_mit(base_edit, &einordnung, &mess, strength, format!("{beschreibung}\n\n{roh}"));
    if let Some(probe) = probe {
        if let Some(text) = nachmessen(&mut r.edit, base_edit, probe) {
            r.zurechtgerueckt.push(text);
            r.geaendert = unterschiede(base_edit, &r.edit);
        }
    }
    Ok(r)
}

/// Der Look (Lichter, Tiefen, Schwarz, Kontrast) verschiebt nebenbei die mittlere
/// Helligkeit — gemessen bis 0,5 EV dunkler als gewollt. Deshalb wird das
/// Ergebnis klein durchgerechnet und die Belichtung so nachgefuehrt, dass die
/// Helligkeit dort liegt, wo Automatik plus gewollte Aenderung sie hinlegen.
fn nachmessen(edit: &mut Edit, basis: &Edit, probe: Probe) -> Option<String> {
    // Ziel mit DERSELBEN Probe messen wie das Ergebnis (gleiche Groesse, gleicher
    // Weg). 50 Reglereinheiten = 1 Blende.
    let (px, w, h) = probe(basis)?;
    let ziel = Messwerte::aus(&px, w, h).helligkeit_ev + (edit.exposure - basis.exposure) / 50.0;
    let start = edit.exposure;
    for _ in 0..2 {
        let (px, w, h) = probe(edit)?;
        let ist = Messwerte::aus(&px, w, h).helligkeit_ev;
        let fehler = ziel - ist;
        if fehler.abs() < 0.04 {
            break;
        }
        edit.exposure = (edit.exposure + fehler * 50.0).clamp(start - 30.0, start + 30.0);
    }
    ((edit.exposure - start).abs() >= 0.5).then(|| {
        tf("exposure: {} -> {} (Helligkeit nachgemessen)", &[&format!("{start:.0}"), &format!("{:.0}", edit.exposure)])
    })
}

/// Der Teil nach dem Modell — getrennt, damit er sich ohne Modell pruefen laesst.
pub fn develop_mit(base_edit: &Edit, einordnung: &Einordnung, mess: &Messwerte, strength: Strength, roh: String) -> AiResult {
    let z = zuschlag(einordnung, mess, strength);
    let mut edit = anwenden(base_edit, &z);
    let mut zurechtgerueckt = Vec::new();
    let mut grenze = |name: &str, wert: &mut f32, mindestens: f32| {
        if *wert < mindestens - 0.01 {
            zurechtgerueckt.push(tf(
                "{}: {} -> {} (Grundeinstellung der Kamera)",
                &[&name, &format!("{wert:.0}"), &format!("{mindestens:.0}")],
            ));
            *wert = mindestens;
        }
    };
    grenze("sharpen", &mut edit.sharpen, base_edit.sharpen);
    grenze("sharpenMask", &mut edit.sharpen_mask, base_edit.sharpen_mask);
    grenze("colorNoise", &mut edit.color_noise, base_edit.color_noise);
    grenze("noise", &mut edit.noise, base_edit.noise);
    // Der automatisch verankerte Schwarzpunkt gehoert zur Grundausstattung:
    // die KI sieht ein kleines Vorschaubild und hat den „Grauschleier" dort
    // schon einmal weggenommen — das Bild kippte zurueck ins Flache.
    grenze("blackpoint", &mut edit.blackpoint, base_edit.blackpoint);
    // Auch die Grundfarbkraft und die gemessene Schatten-Automatik sind
    // Grundausstattung (gegen Referenz-Exporte kalibriert): die KI hat sie
    // regelmaessig HERUNTERGESETZT (vibrance 44 -> 12, shadows 32 -> 5)
    // — das Foto kippte zurueck ins Blasse und die Schatten soffen wieder ab.
    grenze("vibrance", &mut edit.vibrance, base_edit.vibrance);
    grenze("saturation", &mut edit.saturation, base_edit.saturation);
    grenze("shadows", &mut edit.shadows, base_edit.shadows);
    // Leitplanken nach OBEN — gegen ueberzeichnete, ueberstrahlte Ergebnisse:
    // die KI-Nachkontrolle hat wiederholt Extremwerte gesetzt (clarity 28,
    // dehaze 18, sharpen 65, sharpenRadius 85 — Skala verwechselt, gemeint
    // war wohl 0,85). Auf dem 1200-px-Vorschaubild sieht sie die Halos und
    // das Knirschen nicht, im fertigen Foto stehen sie dann. Werte oberhalb
    // der Planke sind nie eine Bildentscheidung, sondern ein Ausrutscher.
    let mut planke = |name: &str, wert: &mut f32, hoechstens: f32| {
        if *wert > hoechstens + 0.01 {
            zurechtgerueckt.push(tf(
                "{}: {} -> {} (Leitplanke gegen Überzeichnung)",
                &[&name, &format!("{wert:.0}"), &format!("{hoechstens:.0}")],
            ));
            *wert = hoechstens;
        }
    };
    planke("sharpen", &mut edit.sharpen, 55.0);
    planke("sharpenRadius", &mut edit.sharpen_radius, 20.0);
    planke("clarity", &mut edit.clarity, 18.0);
    planke("dehaze", &mut edit.dehaze, 12.0);
    planke("texture", &mut edit.texture, 25.0);
    // Die gemessene Lichter-/Weiss-Ruecknahme (negativ) ist ebenfalls
    // Grundausstattung — vertiefen darf die KI sie, zuruecknehmen nicht:
    // sonst brennt der Himmel wieder aus (Weiss viel zu weit aufgedreht).
    // Seit Prozess 2 setzt die Automatik Weiss wie die Referenz-Entwicklung oft
    // auf +12..+30 — die Grenze ist deshalb der Ausgangswert (mindestens +10),
    // nicht +10 fest: sonst senkte jede KI-Runde das Weiss unbemerkt ab.
    planke("whites", &mut edit.whites, base_edit.whites.max(10.0));
    planke("highlights", &mut edit.highlights, base_edit.highlights.max(-70.0));
    // Blaues/cyanes Schatten-Toning ist laut eigener Anweisung verboten
    // („cold, lifeless cast") — die KI setzt es trotzdem regelmaessig
    // (gradeShadowHue 200, Sat 12: Blaustich auf einer weissen Jacke).
    // Anweisungen kann das Modell ignorieren, eine Leitplanke nicht.
    if edit.grade_shadow_sat > 0.0
        && edit.grade_shadow_hue >= 160.0
        && edit.grade_shadow_hue <= 280.0
    {
        zurechtgerueckt.push(tf(
            "gradeShadowSat: {} -> 0 (blaues Schatten-Toning = kalter Stich)",
            &[&format!("{:.0}", edit.grade_shadow_sat)],
        ));
        edit.grade_shadow_sat = 0.0;
    }
    let geaendert = unterschiede(base_edit, &edit);
    AiResult {
        applied: geaendert.iter().map(|g| g.split(':').next().unwrap_or_default().to_string()).collect(),
        edit,
        roh,
        geaendert,
        zurechtgerueckt,
        scene_label: szene_text(einordnung.szene).to_string(),
        look: look_text(einordnung.look).to_string(),
        description: einordnung.beschreibung.clone(),
    }
}

/// Verkleinert und kodiert das Bild für die Anfrage. Kein volles 40-MP-Bild ins Netz:
/// das kostet nur Zeit und Token, ohne die Beurteilung zu verbessern.
fn encode_small_jpeg(
    rgba: &[u8],
    width: u32,
    height: u32,
    max_dim: u32,
) -> Result<Vec<u8>, AiError> {
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| AiError::Encode(t("Puffergröße passt nicht").into()))?;
    let dyn_img = image::DynamicImage::ImageRgba8(img);
    let scaled = if width.max(height) > max_dim {
        dyn_img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
    } else {
        dyn_img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    scaled
        .to_rgb8()
        .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out, 85,
        ))
        .map_err(|e| AiError::Encode(e.to_string()))?;
    Ok(out.into_inner())
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_stimmt() {
        assert_eq!(base64_encode(b"Foto"), "Rm90bw==");
        assert_eq!(base64_encode(b"RAW"), "UkFX");
        assert_eq!(base64_encode(b"a"), "YQ==");
    }
}


/// Nennt die Felder, die sich zwischen zwei Staenden geaendert haben, samt Werten.
///
/// Ueber die Serialisierung, damit die Namen dieselben sind, die auch die KI
/// benutzt — eine zweite Liste von Feldnamen liefe frueher oder spaeter auseinander.
pub fn unterschiede(vorher: &Edit, nachher: &Edit) -> Vec<String> {
    let (Ok(serde_json::Value::Object(a)), Ok(serde_json::Value::Object(b))) =
        (serde_json::to_value(vorher), serde_json::to_value(nachher))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, vb) in &b {
        // Die gemessene Grundhelligkeit ist keine Entscheidung der KI.
        if k == "baseEv" {
            continue;
        }
        match a.get(k) {
            Some(va) if va == vb => {}
            Some(va) => out.push(format!("{k}: {} -> {}", kurz(va), kurz(vb))),
            None => out.push(format!("{k}: {}", kurz(vb))),
        }
    }
    out.sort();
    out
}

fn kurz(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Number(n) => {
            let f = n.as_f64().unwrap_or(0.0);
            if f.fract().abs() < 0.05 {
                format!("{f:.0}")
            } else {
                format!("{f:.1}")
            }
        }
        serde_json::Value::Array(a) if a.len() > 3 => tf("[{} Werte]", &[&a.len()]),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod unterschied_tests {
    use super::*;

    #[test]
    fn nur_echte_aenderungen_werden_genannt() {
        let a = Edit::default();
        let mut b = a.clone();
        b.exposure = 12.0;
        b.base_ev = 1.9; // gemessen, keine Entscheidung der KI
        let d = unterschiede(&a, &b);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].starts_with("exposure: 0 -> 12"), "{d:?}");
    }

    #[test]
    fn gleichstand_ergibt_leere_liste() {
        let a = Edit::default();
        assert!(unterschiede(&a, &a.clone()).is_empty());
    }
}

#[cfg(test)]
mod lokal_tests {
    use super::*;

    #[test]
    fn einordnung_liest_auch_kaputtes_json() {
        // So kam es gemessen vom Modell: `\"` vor dem Doppelpunkt.
        let roh = r#"{"scene":"landscape","light":"direct_sun","people":"none","sky":"large_part","haze":"no","brightness":"slightly_dark","colour_cast":"none_or_intended","look":"natural","description_de\":\"Eine ruhige Flusslandschaft.\"}"#;
        let e = Einordnung::lesen(roh).unwrap();
        assert_eq!(e.szene, "landscape");
        assert_eq!(e.himmel, "large_part");
        assert_eq!(e.helligkeit, "slightly_dark");
        assert!(!e.dunst);
        assert_eq!(e.beschreibung, "Eine ruhige Flusslandschaft.");
    }

    #[test]
    fn beschreibung_unter_neuem_schluessel() {
        let roh = r#"{"scene":"night","description":"A quiet street at night."}"#;
        assert_eq!(Einordnung::lesen(roh).unwrap().beschreibung, "A quiet street at night.");
    }

    #[test]
    fn ohne_szene_gibt_es_keine_einordnung() {
        assert!(Einordnung::lesen("Ich sehe ein Foto.").is_none());
    }

    fn einordnung(szene: &'static str) -> Einordnung {
        Einordnung {
            szene,
            licht: "direct_sun",
            menschen: "none",
            himmel: "none",
            dunst: false,
            helligkeit: "right",
            stich: "none_or_intended",
            look: "natural",
            beschreibung: String::new(),
        }
    }

    const MITTEL: Messwerte = Messwerte { helligkeit_ev: -0.9, lichter_prozent: 0.2, warm_kalt: 0.0, gruen_magenta: 0.0 };

    #[test]
    fn landschaft_wird_lebendiger_als_ein_portraet() {
        let l = zuschlag(&einordnung("landscape"), &MITTEL, Strength::Normal);
        let p = zuschlag(&einordnung("portrait"), &MITTEL, Strength::Normal);
        assert!(l.vibrance > p.vibrance && l.clarity > p.clarity);
    }

    #[test]
    fn staerke_skaliert_den_look() {
        let s = zuschlag(&einordnung("landscape"), &MITTEL, Strength::Sanft);
        let k = zuschlag(&einordnung("landscape"), &MITTEL, Strength::Kraeftig);
        assert!(k.vibrance > s.vibrance * 2.0);
    }

    #[test]
    fn helligkeit_nur_wenn_die_messung_zustimmt() {
        let mut e = einordnung("landscape");
        e.helligkeit = "too_dark";
        let hell = Messwerte { helligkeit_ev: -0.2, ..MITTEL };
        assert_eq!(zuschlag(&e, &hell, Strength::Normal).exposure, 0.0, "Modell irrt, Bild ist hell");
        let dunkel = Messwerte { helligkeit_ev: -2.0, ..MITTEL };
        assert!(zuschlag(&e, &dunkel, Strength::Normal).exposure > 0.0);
    }

    #[test]
    fn gewolltes_warmes_licht_bleibt_warm() {
        let mut e = einordnung("sunset_or_sunrise");
        e.stich = "too_warm_yellow";
        let warm = Messwerte { warm_kalt: 20.0, ..MITTEL };
        assert!(zuschlag(&e, &warm, Strength::Normal).temp >= 0.0);
    }

    #[test]
    fn gesichter_bekommen_keine_harte_struktur() {
        let mut e = einordnung("city_or_architecture");
        e.menschen = "prominent_faces";
        let z = zuschlag(&e, &MITTEL, Strength::Kraeftig);
        assert!(z.clarity <= 0.0 && z.texture <= 0.0);
    }

    #[test]
    fn ausgefressene_lichter_werden_zurueckgenommen() {
        let e = einordnung("other");
        let hell = Messwerte { lichter_prozent: 4.0, ..MITTEL };
        assert!(zuschlag(&e, &hell, Strength::Normal).highlights < zuschlag(&e, &MITTEL, Strength::Normal).highlights - 4.0);
    }

    #[test]
    fn leitplanken_gelten_weiter() {
        let mut basis = Edit::default();
        basis.vibrance = 30.0;
        let mut e = einordnung("portrait");
        e.look = "soft";
        let r = develop_mit(&basis, &e, &MITTEL, Strength::Kraeftig, String::new());
        assert!(r.edit.vibrance >= 30.0, "Grundfarbkraft darf nicht sinken");
        assert!(r.edit.clarity <= 18.0);
    }

    #[test]
    fn messwerte_eines_grauen_bildes() {
        let px = vec![128u8; 64 * 64 * 4];
        let m = Messwerte::aus(&px, 64, 64);
        assert!(m.lichter_prozent == 0.0);
        assert!(m.warm_kalt.abs() < 0.01);
        // sRGB 128 = 21,6 % linear -> knapp ueber Mittelgrau
        assert!((m.helligkeit_ev - 0.26).abs() < 0.05, "{}", m.helligkeit_ev);
    }
}
