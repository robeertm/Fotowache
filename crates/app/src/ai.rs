//! KI-Anbindung — derselbe Vertrag wie in der Web-Version.
//!
//! Der Text der Anweisung ist **wörtlich aus `site/app.js` übernommen** (`AI_KEYS_SPEC`,
//! `AI_ASK_BASE`). Das ist Absicht: der Wortlaut wurde über viele Runden
//! geschliffen (kein Grauschleier, nie zu warm, nie matschig). Ein neu erfundener
//! Prompt würde anders entwickeln — und die Web-Version und das Programm lieferten
//! plötzlich verschiedene Ergebnisse für dasselbe Foto. Deshalb wird hier auch
//! nichts übersetzt: was an den Server geht, bleibt in jeder Sprache gleich. Nur
//! die Meldungen für die Oberfläche laufen über `sprache::t`.
//!
//! Weg der Anfrage: derselbe KI-Server wie in der Web-Version (`/api/ai/develop`).
//! Der Zugang zum Modell liegt server-seitig; im Browser ging das nicht (der
//! KI-Anbieter sperrt CORS für Abo-Organisationen), und auch hier ist es der
//! bessere Weg — so gibt es genau eine Stelle, an der ein Token liegt, statt
//! einer Kopie auf jedem Rechner.

use crate::sprache::{t, tf};
use pipeline::params::Edit;

/// Erlaubte Regler samt Wertebereichen — wörtlich aus der Web-Version.
const AI_KEYS_SPEC: &str = concat!(
    "Allowed keys (all optional, integers, ranges in brackets):\n",
    "exposure[-100..100], contrast[-100..100], highlights[-100..100], shadows[-100..100], whites[-100..100], blacks[-100..100],\n",
    // Symmetrisch formuliert. Vorher stand hier nur „never yellow/orange" — die KI
    // wich der einen genannten Gefahr aus und landete regelmaessig auf der anderen
    // Seite: die Fotos kamen oft zu kuehl heraus.
    "temp[-100..100 warmer=+; keep near 0. Whites must be NEUTRAL white — avoid a warm/yellow/orange cast AND equally a cool/blue/cyan cast. Neither direction is 'safer' than the other],\n",
    "tint[-100..100 magenta=+], vibrance[-100..100], saturation[-40..60 keep colour, never grey],\n",
    "clarity[-100..100], texture[-100..100], dehaze[-100..100], vignette[-100..100], grain[0..100],\n",
    // Diese fuenf sind Aufnahme-Grundeinstellungen, kein Look. Die KI hat sie
    // regelmaessig HERUNTERGESETZT (Schaerfe 38 -> 25, Farbrauschen 22 -> 5) und
    // damit die Grundschaerfung wieder weggenommen.
    "sharpen[0..100], sharpenRadius[0..100], sharpenMask[0..100], noise[0..100], colorNoise[0..100] — these are ALREADY set to correct capture defaults for this camera. Raise them only if this photo genuinely needs more; NEVER lower them.\n",
    "gradeShadowHue[0..360], gradeShadowSat[0..100], gradeMidHue[0..360], gradeMidSat[0..100], gradeHighHue[0..360], gradeHighSat[0..100], gradeBalance[-100..100],\n",
    "hslH, hslS, hslL = arrays of 8 integers[-100..100] for [red,orange,yellow,green,cyan,blue,violet,magenta],\n",
    "curve = array of 5 increasing numbers[0..1], output at inputs 0,0.25,0.5,0.75,1; MUST rise dark→bright, neutral = [0,0.25,0.5,0.75,1]; never inverted like [1,0.75,0.5,0.25,0]. Omit if unsure.\n",
);

const AI_ASK_BASE: &str = concat!(
    "Analyse this photo and return development parameters as ONE JSON object. ",
    "Make it a natural, positive COLOUR photo — never a negative and never black & white.\n",
);

const AI_ASK_TAIL: &str = concat!(
    "Express your whole edit through these keys: besides exposure & white balance, also set the creative controls you would actually use for this photo — contrast, highlights/shadows, clarity, texture, vibrance, and where it helps the mood also HSL per colour and split-toning. That way every slider reflects your development and the user can fine-tune it. Keep it natural, never extreme.\n",
    "IMPORTANT — keep the photo crisp and true, never muddy or washed out: keep noise reduction (noise) low (0..20) so you never blur away detail; do NOT push contrast, clarity, texture or dehaze to extremes; avoid a flat, grey, over-smoothed or over-sharpened look. When in doubt, edit gently.\n",
    // Gemessen: die KI hob auf einem Wiesenbild Dynamik +12, Saettigung +8 UND
    // zusaetzlich Gruen +10 und Gelb +8 an. Das summiert sich, und das Foto kippt
    // ins Gruene — ein sichtbarer Gruenstich.
    "DO NOT STACK COLOUR BOOSTS: vibrance and saturation already lift every colour. On top of that, do NOT also raise hslS for the colour family that already dominates the frame (green foliage/grass, blue sky, orange skin) — the whole photo then takes on that colour. Use hslS mainly to hold a colour BACK, or to lift a colour that is under-represented.\n",
    "COLOUR CAST — the photo must look neutral, not tinted. Split-toning (gradeShadow*, gradeMid*, gradeHigh*) is for a DELIBERATE look only. Do NOT tint the shadows blue or cyan to make the photo look 'clean' or 'modern' — that reads as a cold, lifeless cast. If you are not deliberately going for a warm or cool look, leave ALL grade* values at 0.\n",
    "CRUCIAL — NEVER leave a grey veil / milky haze: the deepest shadows must reach a proper near-black (a clean black point), the image must never look faded, washed-out or foggy. Do NOT lift 'blacks' or 'shadows' so far that darks turn grey. If anything, secure the black point (negative 'blacks' or a slightly S-shaped 'curve') so the photo stays punchy and clear.\n",
    "Also include THREE short text fields, all written in natural GERMAN:\n",
    "  \"sceneLabel\" — very short scene tag (max ~6 words),\n",
    "  \"look\" — the look you chose, e.g. \"natürlich & frisch\",\n",
    "  \"description\" — ONE or TWO friendly sentences describing what is in the photo. Under 200 characters, no markdown.\n",
    "Answer with the JSON object only, no prose around it.\n",
);

/// Wie kräftig die KI entwickeln soll — wie der Stärke-Regler der Web-Version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Sanft,
    Normal,
    Kraeftig,
}

impl Strength {
    fn hint(self) -> &'static str {
        match self {
            Strength::Sanft => "\nEDIT STRENGTH: The user wants a SUBTLE, restrained edit — correct what is wrong, add little.",
            Strength::Normal => "",
            Strength::Kraeftig => "\nEDIT STRENGTH: The user wants a STRONG, punchy, expressive edit — be more assertive with exposure, contrast, clarity, dehaze and colour, while staying believable and never unnatural, blown-out or muddy.",
        }
    }
    /// Fuer die Oberflaeche — an den Server geht nur `hint()`.
    pub fn label(self) -> &'static str {
        match self {
            Strength::Sanft => t("sanft"),
            Strength::Normal => t("normal"),
            Strength::Kraeftig => t("kräftig"),
        }
    }
}

/// Was die KI zurückgemeldet hat — Regler plus die drei Beschreibungsfelder.
#[derive(Debug, Clone, Default)]
pub struct AiResult {
    pub edit: Edit,
    pub applied: Vec<String>,
    pub scene_label: String,
    pub look: String,
    pub description: String,
    /// Die Antwort der KI im Wortlaut — fuers Protokoll. Ohne sie steht bei einer
    /// merkwuerdigen Entwicklung nur „25 Regler gesetzt" da und man weiss nicht,
    /// ob die KI Unsinn geschickt oder der Empfang etwas verschluckt hat.
    pub roh: String,
    /// Welche Werte sich tatsaechlich geaendert haben, als Text.
    pub geaendert: Vec<String>,
    /// Was gegen die Untergrenzen der Aufnahme-Grundeinstellungen verstiess und
    /// zurechtgerueckt wurde. Steht im Protokoll.
    pub zurechtgerueckt: Vec<String>,
}

#[derive(Debug)]
pub enum AiError {
    Net(String),
    Http { code: u16, body: String },
    Parse,
    Encode(String),
}

/// Von Hand statt ueber `thiserror`: die Meldung landet in der Oberflaeche und
/// entsteht deshalb erst beim Anzeigen, in der gerade gewaehlten Sprache.
impl std::fmt::Display for AiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            AiError::Net(e) => tf("Der KI-Server ist nicht erreichbar: {}", &[e]),
            AiError::Http { code, body } => tf("Der KI-Server hat abgelehnt ({}): {}", &[code, body]),
            AiError::Parse => t("Antwort war kein verwertbares JSON").to_string(),
            AiError::Encode(e) => tf("Bild konnte nicht verpackt werden: {}", &[e]),
        };
        f.write_str(&text)
    }
}

impl std::error::Error for AiError {}

/// Anweisung für die Nachkontrolle — die KI sieht ihr eigenes Ergebnis und
/// korrigiert nach. Inhaltlich aus der Web-Version übernommen (dort „Feinschliff").
const AI_REVIEW: &str = concat!(
    "This is the SAME photo after your development. Judge it critically against high-end ",
    "professional standards: correct exposure, natural NEUTRAL white balance (whites white, ",
    "skin natural — neither a warm/yellow/orange cast NOR a cool/blue/cyan one; check the ",
    "shadows in particular for a blue tinge), no grey veil, no muddy or over-smoothed ",
    "look, no clipped highlights, no blocked shadows, colours rich but believable.\n",
    "Return ONE JSON object with the CORRECTED full set of parameters (same keys as before). ",
    "If something is already right, keep its value. Only change what genuinely needs changing — ",
    "small, deliberate corrections, not a new look.\n",
);

/// Fragt die KI und gibt die zusammengeführten Regler zurück.
///
/// `base_edit` ist der aktuelle Stand — die KI-Antwort wird daraufgelegt, statt alles
/// zu ersetzen. Nur genannte Schlüssel ändern sich (siehe `Edit::merge_json`).
pub fn develop(
    endpoint: &str,
    rgba: &[u8],
    width: u32,
    height: u32,
    base_edit: &Edit,
    strength: Strength,
    review: bool,
) -> Result<AiResult, AiError> {
    let jpeg = encode_small_jpeg(rgba, width, height, 1200)?;
    let b64 = base64_encode(&jpeg);

    let ask = if review {
        format!("{AI_REVIEW}{AI_KEYS_SPEC}{}", strength.hint())
    } else {
        format!("{AI_ASK_BASE}{AI_KEYS_SPEC}{AI_ASK_TAIL}{}", strength.hint())
    };
    let body = serde_json::json!({
        "imageBase64": b64,
        "mime": "image/jpeg",
        "maxTokens": 1400,
        "user": ask,
    });

    let url = format!("{}/api/ai/develop", endpoint.trim_end_matches('/'));
    let resp = ureq::post(&url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(90)))
        .build()
        .send_json(&body);

    let mut resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(code)) => {
            return Err(AiError::Http {
                code,
                body: t("siehe Server-Protokoll").into(),
            })
        }
        Err(e) => return Err(AiError::Net(e.to_string())),
    };

    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| AiError::Net(e.to_string()))?;
    let outer: serde_json::Value = serde_json::from_str(&text).map_err(|_| AiError::Parse)?;

    // Der Server antwortet {"ok":true,"text":"…"} — im Text steckt das eigentliche JSON.
    let inner_text = outer
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or(AiError::Parse)?;
    let json = extract_json(inner_text).ok_or(AiError::Parse)?;

    let mut edit = base_edit.clone();
    let applied = edit.merge_json(&json);

    // Untergrenze fuer die Aufnahme-Grundeinstellungen.
    //
    // Die KI beurteilt ein 1200-px-Vorschaubild. Auf dieser Groesse ist eine
    // Schaerfung mit einem Pixel Radius schlicht nicht zu sehen — sie kann also
    // gar nicht sinnvoll darueber entscheiden und hat sie regelmaessig gesenkt
    // (38 -> 25) und damit die Grundschaerfung wieder weggenommen. Anheben darf
    // sie weiterhin; senken nicht. Was dabei zurechtgerueckt wurde, steht im
    // Protokoll — heimlich uebergehen waere schlimmer als der Fehler selbst.
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
    // Was hat sich wirklich bewegt? Die KI nennt oft Werte, die schon so standen —
    // „25 Regler gesetzt" saehe dann nach viel Arbeit aus, wo nichts passierte.
    let geaendert = unterschiede(base_edit, &edit);
    Ok(AiResult {
        edit,
        applied,
        roh: serde_json::to_string_pretty(&json).unwrap_or_else(|_| inner_text.to_string()),
        geaendert,
        zurechtgerueckt,
        scene_label: json
            .get("sceneLabel")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        look: json
            .get("look")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        description: json
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// Modelle packen ihre Antwort gern in Fließtext oder Code-Zäune. Wir schneiden das
/// äußerste geschweifte Klammernpaar heraus, statt auf sauberes JSON zu hoffen.
fn extract_json(s: &str) -> Option<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(s.trim()) {
        return Some(v);
    }
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&s[start..=end]).ok()
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

fn base64_encode(data: &[u8]) -> String {
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

    #[test]
    fn json_aus_fliesstext() {
        let v = extract_json("Hier ist das Ergebnis:\n```json\n{\"exposure\": 5}\n```\nViel Spaß")
            .unwrap();
        assert_eq!(v["exposure"], 5);
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
