//! „Raw-Profile" anderer Anwendungen (Looks: Farbe, Portraet, Landschaft,
//! Neutral, Lebendig — Namen siehe `ANGEBOTEN`).
//!
//! Die Referenz-Entwicklung entwickelt seit 2018 standardmaessig mit dem
//! Farb-Look — das ist kein eigenes Kameraprofil, sondern das installierte
//! Standardprofil (die DCP der Kamera) plus eine Farbtabelle (HSV-„Look") und
//! eine leichte Gradationskurve, beschrieben in einer XMP-Datei im
//! Programmordner der Referenz-Entwicklung (Unterordner siehe `ordner()`,
//! Dateiname = Name des Looks + `.xmp`).
//!
//! Die Tabelle steht dort als `crs:Table_<Fingerabdruck>="…"`, kodiert wie im
//! DNG-SDK (`dng_big_table::ASCIItoBinary` / `DecodeFromBinary`):
//! eine Z85-aehnliche Base85-Variante (eigene Zeichentabelle, 5 Zeichen → 4 Byte,
//! Little Endian), darin die unkomprimierte Laenge (4 Byte LE) und ein
//! zlib-Strom, darin: Typ (0 = LookTable), Version (1/2), Teilungen fuer
//! Farbton/Saettigung/Wert, je Stuetzstelle (Farbtonverschiebung in Grad,
//! Saettigungsfaktor, Wertfaktor) als f32, dann die Kodierung der Wertachse
//! (0 linear, 1 sRGB). Reihenfolge der Stuetzstellen wie bei DCP-Tabellen:
//! Wert aussen, dann Farbton, Saettigung innen.
//!
//! Gelesen wird nur, was eine andere Anwendung auf DIESEM Rechner installiert
//! hat — mitgeliefert wird nichts.

use crate::dcp::{Tabelle, STANDARDPROFIL};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

/// Ein Raw-Profil („Look") einer anderen Anwendung, fertig dekodiert.
#[derive(Debug, Clone)]
pub struct LookProfil {
    pub name: String,
    /// Auf welchem Kameraprofil der Look aufsetzt (immer das installierte
    /// Standardprofil, `STANDARDPROFIL`).
    pub basisprofil: String,
    pub tabelle: Tabelle,
    /// Gradationskurve des Looks (ToneCurvePV2012, 0..255 → 0..255).
    pub kurve: Vec<(f32, f32)>,
    /// Wandelt der Look in Graustufen (Schwarzweiss-Look)?
    pub graustufen: bool,
}

/// Die Namen der Raw-Profile, die Fotowache anbietet (der Schwarzweiss-Look nicht:
/// das ist eine Schwarzweiss-Umsetzung, dafuer gibt es den eigenen Schalter).
/// Zugleich die Dateinamen der Looks und die gespeicherten Profilschluessel.
pub const ANGEBOTEN: [&str; 5] = ["Adobe Color", "Adobe Portrait", "Adobe Landscape", "Adobe Neutral", "Adobe Vivid"]; // Kennung: Dateinamen, gespeicherte Schluessel

/// Ordner, in denen andere Anwendungen die Raw-Profile ablegen: jeweils neben
/// dem CameraProfiles-Ordner, den auch die DCP-Suche kennt.
pub fn ordner() -> Vec<PathBuf> {
    let mut v = Vec::new();
    for ort in crate::dcp::suchorte() {
        if let Some(eltern) = ort.parent() {
            let p = eltern.join("Settings/Adobe/Profiles/Adobe Raw"); // Kennung: Suchpfad
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v
}

static SPEICHER: OnceLock<Mutex<HashMap<String, Option<Arc<LookProfil>>>>> = OnceLock::new();

/// Den Look mit diesem Namen laden (einmal je Lauf, danach aus dem Speicher).
pub fn laden(name: &str) -> Option<Arc<LookProfil>> {
    if std::env::var_os("FOTOWACHE_OHNE_FREMDPROFILE").is_some() {
        return None;
    }
    let speicher = SPEICHER.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(l) = speicher.lock().ok().and_then(|m| m.get(name).cloned()) {
        return l;
    }
    let gefunden = ordner()
        .into_iter()
        .map(|o| o.join(format!("{name}.xmp")))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| aus_xmp(name, &text))
        .map(Arc::new);
    if let Ok(mut m) = speicher.lock() {
        m.insert(name.to_string(), gefunden.clone());
    }
    gefunden
}

/// Wert eines XMP-Attributs `crs:<name>="…"`.
fn attribut<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let muster = format!("crs:{name}=\"");
    let a = text.find(&muster)? + muster.len();
    let e = text[a..].find('"')?;
    Some(&text[a..a + e])
}

/// Die Punkte einer `<crs:ToneCurvePV2012>`-Folge.
fn kurve_lesen(text: &str) -> Vec<(f32, f32)> {
    let Some(a) = text.find("<crs:ToneCurvePV2012>") else { return Vec::new() };
    let Some(e) = text[a..].find("</crs:ToneCurvePV2012>") else { return Vec::new() };
    text[a..a + e]
        .split("<rdf:li>")
        .skip(1)
        .filter_map(|teil| {
            let wert = teil.split("</rdf:li>").next()?;
            let mut it = wert.split(',').map(|z| z.trim().parse::<f32>());
            Some((it.next()?.ok()?, it.next()?.ok()?))
        })
        .collect()
}

/// Einen Look aus dem Text seiner XMP-Datei lesen.
pub fn aus_xmp(name: &str, text: &str) -> Option<LookProfil> {
    let basisprofil = attribut(text, "CameraProfile").unwrap_or(STANDARDPROFIL).to_string();
    let fp = attribut(text, "LookTable")?;
    let kodiert = attribut(text, &format!("Table_{fp}"))?;
    let tabelle = tabelle_dekodieren(kodiert)?;
    Some(LookProfil {
        name: name.to_string(),
        basisprofil,
        tabelle,
        kurve: kurve_lesen(text),
        graustufen: attribut(text, "ConvertToGrayscale").is_some_and(|v| v.eq_ignore_ascii_case("true")),
    })
}

/// Die Base85-Variante des DNG-SDK (`ASCIItoBinary`): Zeichen 32..127 → Ziffer
/// 0..84, 0xFF = ueberspringen.
fn base85(text: &str) -> Vec<u8> {
    const TAFEL: [u8; 96] = [
        0xFF, 0x44, 0xFF, 0x54, 0x53, 0x52, 0xFF, 0x49, 0x4B, 0x4C, 0x46, 0x41, 0xFF, 0x3F, 0x3E, 0x45, // ␣ … /
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, // 0 … 9
        0x40, 0xFF, 0xFF, 0x42, 0xFF, 0x47, 0x51, // : ; < = > ? @
        0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, // A … M
        0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, // N … Z
        0x4D, 0xFF, 0x4E, 0x43, 0xFF, 0x48, // [ \ ] ^ _ `
        0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, // a … m
        0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, // n … z
        0x4F, 0x4A, 0x50, 0xFF, 0xFF, // { | } ~ DEL
    ];
    let mut aus = Vec::with_capacity(text.len() / 5 * 4 + 4);
    let (mut phase, mut wert) = (0u32, 0u32);
    for b in text.bytes() {
        if !(32..=127).contains(&b) {
            continue;
        }
        let d = TAFEL[(b - 32) as usize] as u32;
        if d > 85 {
            continue;
        }
        phase += 1;
        match phase {
            1 => wert = d,
            2 => wert = wert.wrapping_add(d * 85),
            3 => wert = wert.wrapping_add(d * 85 * 85),
            4 => wert = wert.wrapping_add(d * 85 * 85 * 85),
            _ => {
                wert = wert.wrapping_add(d.wrapping_mul(85 * 85 * 85 * 85));
                aus.extend_from_slice(&wert.to_le_bytes());
                phase = 0;
            }
        }
    }
    // Rest wie im SDK: je angefangener Gruppe die niedrigen Bytes.
    if phase > 1 {
        aus.push(wert as u8);
    }
    if phase > 2 {
        aus.push((wert >> 8) as u8);
    }
    if phase > 3 {
        aus.push((wert >> 16) as u8);
    }
    aus
}

/// Base85 → zlib → LookTable-Strom → `Tabelle`.
pub fn tabelle_dekodieren(text: &str) -> Option<Tabelle> {
    let roh = base85(text);
    if roh.len() < 8 {
        return None;
    }
    let laenge = u32::from_le_bytes([roh[0], roh[1], roh[2], roh[3]]) as usize;
    if laenge > 16 * 1024 * 1024 {
        return None;
    }
    let mut daten = Vec::with_capacity(laenge);
    flate2::read::ZlibDecoder::new(&roh[4..]).read_to_end(&mut daten).ok()?;
    let u32_bei = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(daten.get(o..o + 4)?.try_into().ok()?)) };
    if u32_bei(0)? != 0 {
        return None; // kein LookTable
    }
    let version = u32_bei(4)?;
    if version != 1 && version != 2 {
        return None;
    }
    let (h, s, v) = (u32_bei(8)?, u32_bei(12)?, u32_bei(16)?);
    if h < 1 || s < 2 || v < 1 || h > 360 || s > 256 || v > 256 || h * s * v > 36 * 32 * 16 {
        return None;
    }
    let n = (h * s * v) as usize * 3;
    let mut werte = Vec::with_capacity(n);
    for i in 0..n {
        werte.push(f32::from_bits(u32_bei(20 + i * 4)?));
    }
    let kodierung = u32_bei(20 + n * 4)?;
    if kodierung > 1 || werte.iter().any(|w| !w.is_finite()) {
        return None;
    }
    Some(Tabelle {
        farbtoene: h,
        saettigungen: s,
        werte: v,
        kodierung,
        daten: werte,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kodierer nach dem DNG-SDK (`BinarytoASCII`) — nur fuer den Test, damit
    /// der Leser an einer selbst gebauten Tabelle beweisen kann, dass Hin- und
    /// Rueckweg dieselben Zahlen ergeben.
    fn base85_kodieren(daten: &[u8]) -> String {
        const ZEICHEN: &[u8; 85] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";
        let mut aus = String::new();
        for kette in daten.chunks(4) {
            let mut b = [0u8; 4];
            b[..kette.len()].copy_from_slice(kette);
            let mut w = u32::from_le_bytes(b);
            let stellen = kette.len() + 1;
            for _ in 0..stellen {
                aus.push(ZEICHEN[(w % 85) as usize] as char);
                w /= 85;
            }
        }
        aus
    }

    #[test]
    fn look_tabelle_hin_und_zurueck() {
        use std::io::Write;
        let (h, s, v) = (6u32, 3u32, 2u32);
        let mut strom: Vec<u8> = Vec::new();
        for z in [0u32, 1, h, s, v] {
            strom.extend(z.to_le_bytes());
        }
        let mut erwartet = Vec::new();
        for i in 0..(h * s * v) {
            for w in [i as f32 * 0.5 - 3.0, 1.0 + i as f32 * 0.01, 0.9] {
                strom.extend(w.to_le_bytes());
                erwartet.push(w);
            }
        }
        strom.extend(0u32.to_le_bytes()); // linear
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&strom).unwrap();
        let mut block = (strom.len() as u32).to_le_bytes().to_vec();
        block.extend(z.finish().unwrap());
        let t = tabelle_dekodieren(&base85_kodieren(&block)).expect("Tabelle");
        assert_eq!((t.farbtoene, t.saettigungen, t.werte, t.kodierung), (h, s, v, 0));
        assert_eq!(t.daten, erwartet);
    }

    #[test]
    fn kurve_und_attribute() {
        let x = r#"<rdf:Description crs:CameraProfile="Testprofil" crs:LookTable="AB">
            <crs:ToneCurvePV2012><rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>22, 16</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq></crs:ToneCurvePV2012>"#;
        assert_eq!(attribut(x, "LookTable"), Some("AB"));
        assert_eq!(kurve_lesen(x), vec![(0.0, 0.0), (22.0, 16.0), (255.0, 255.0)]);
    }
}
