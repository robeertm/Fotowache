//! Objektivprofile fuer ALLE Kameras: lensfun (mitgeliefert) und installierte
//! Objektivprofile (LCP, von anderen Anwendungen mitgebracht).
//!
//! Beide Quellen beschreiben dasselbe — Verzeichnung, Farbsaeume und
//! Vignettierung als Funktionen des Bildradius, je Brennweite (und Blende) —
//! nur in verschiedenen Modellen und Koordinaten. Hier werden sie in die EINE
//! Form gebracht, die der Schader kennt: `objektiv::Radial`, radiale
//! Stuetzstellen relativ zur halben Bilddiagonale. Dieselbe Form fuellt auch
//! die Kamera-Korrektur der RAF — es gibt nur einen Korrekturweg.
//!
//! **lensfun:** Datenbank CC BY-SA 3.0 (`daten/LENSFUN-LICENSE.txt`), als
//! komprimierter Schnappschuss eingebettet (`scripts/lensfun_aktualisieren.py`).
//! Nur die Daten werden genutzt; die Modelle sind hier selbst umgesetzt:
//! - Verzeichnung/Farbsaum in Hugin-Koordinaten: r = 1 auf halber kurzer Seite
//!   des KALIBRIER-Sensors; ptlens r_q = r(a r³ + b r² + c r + 1−a−b−c),
//!   poly3 r_q = r(1 − k1 + k1 r²), poly5 r_q = r(1 + k1 r² + k2 r⁴).
//! - Farbsaum linear (Massstab v) oder poly3 (b r² + c r + v) je Rot/Blau.
//! - Vignettierung „pa": Helligkeit · (1 + k1 r² + k2 r⁴ + k3 r⁶), r = 1 in der
//!   Ecke des Kalibrier-Sensors.
//!
//! **Installierte LCP:** das Kameramodell des LCP-Formats — Koordinaten in
//! Brennweiten-Einheiten (fx = Brennweite normiert auf die laengere Bildseite),
//! Verzeichnung 1 + k1 r² + k2 r⁴ + k3 r⁶, Vignettierung ebenso (oder
//! stueckweise), Farbsaum als eigenes Modell je Rot/Blau gegen Gruen.

use crate::objektiv::{Radial, RADIAL_MAX};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Gemeinsam: aus Funktionen eine Stuetzstellen-Tabelle machen
// ---------------------------------------------------------------------------

/// Baut eine `Radial`-Tabelle. Alle Funktionen bekommen den Radius als Anteil
/// der halben Bilddiagonale des FOTOS:
/// - `m(r_ziel)`: Quelle = m · Ziel (Verzeichnung, Ziel = korrigiertes Bild)
/// - `ca_r/ca_b(r_quelle)`: Massstab-Abweichung Rot/Blau gegen Gruen
/// - `gain(r_quelle)`: Helligkeitsausgleich der Vignettierung
pub fn radial_aus(
    breite: u32,
    hoehe: u32,
    m: &dyn Fn(f32) -> f32,
    ca_r: &dyn Fn(f32) -> f32,
    ca_b: &dyn Fn(f32) -> f32,
    gain: &dyn Fn(f32) -> f32,
) -> Option<Radial> {
    let n = RADIAL_MAX;
    let mut r = Radial {
        n,
        r_quelle: [0.0; RADIAL_MAX],
        r_ziel: [0.0; RADIAL_MAX],
        m: [1.0; RADIAL_MAX],
        ca_r: [0.0; RADIAL_MAX],
        ca_b: [0.0; RADIAL_MAX],
        gain: [1.0; RADIAL_MAX],
        fuell: 1.0,
    };
    for k in 0..n {
        // Dichter zum Rand hin — dort aendern sich die Polynome am schnellsten.
        let rz = 1.06 * (k as f32 / (n - 1) as f32).sqrt();
        let mm = if k == 0 { m(0.0) } else { m(rz) };
        if !mm.is_finite() || mm <= 0.3 || mm > 3.0 {
            return None;
        }
        r.r_ziel[k] = rz;
        r.m[k] = mm;
        r.r_quelle[k] = rz * mm;
    }
    if !r.r_quelle.windows(2).all(|w| w[1] > w[0] || w[0] == 0.0 && w[1] == 0.0) {
        return None; // nicht umkehrbar — lieber nicht korrigieren
    }
    for k in 0..n {
        let rq = r.r_quelle[k];
        r.ca_r[k] = ca_r(rq).clamp(-0.02, 0.02);
        r.ca_b[k] = ca_b(rq).clamp(-0.02, 0.02);
        r.gain[k] = gain(rq).clamp(0.5, 4.0);
    }
    r.fuell = fuell(&r, breite, hoehe);
    Some(r)
}

fn fuell(r: &Radial, breite: u32, hoehe: u32) -> f32 {
    let (hw, hh) = (breite as f32 / 2.0, hoehe as f32 / 2.0);
    let diag = (hw * hw + hh * hh).sqrt().max(1.0);
    let mut groesste = 1.0f32;
    for i in 0..=64 {
        let t = i as f32 / 64.0;
        for (x, y) in [(hw * (2.0 * t - 1.0), hh), (hw, hh * (2.0 * t - 1.0))] {
            let m = r.m_an((x * x + y * y).sqrt() / diag);
            groesste = groesste.max((m * x.abs() / hw).max(m * y.abs() / hh));
        }
    }
    1.0 / groesste
}

/// Vergleichsform eines Objektivnamens: nur Buchstaben/Ziffern, klein.
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Brennweitenbereich und Lichtstaerke aus einem Objektivnamen
/// („18-300mm F3.5-6.3" → (18, 300, 3.5)).
fn eckdaten(name: &str) -> Option<(f32, f32, f32)> {
    // Gedankenstriche („18–55mm") wie Bindestriche behandeln.
    let s = name
        .to_ascii_lowercase()
        .replace(',', ".")
        .replace(['\u{2013}', '\u{2014}'], "-");
    let pos_mm = s.find("mm")?;
    let vor = &s[..pos_mm];
    let start = vor
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-'))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    let bereich = &vor[start..];
    let mut teile = bereich.split('-').filter_map(|t| t.parse::<f32>().ok());
    let f0 = teile.next()?;
    let f1 = teile.next().unwrap_or(f0);
    // Lichtstaerke: erste Zahl nach „f" (f/2.8, F4, 1:2.8)
    let nach = &s[pos_mm..];
    let mut blende = 0.0;
    for (i, c) in nach.char_indices() {
        if c == 'f' || c == ':' {
            let rest: String = nach[i + 1..]
                .trim_start_matches(['/', ' '])
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = rest.parse::<f32>() {
                blende = v;
                break;
            }
        }
    }
    Some((f0, f1, blende))
}

fn woerter(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .collect()
}

/// Wie gut passt ein Datenbankname zum Objektivnamen aus der Datei? 0 = gar nicht.
pub fn passung(datei: &str, kandidat: &str) -> f32 {
    let (a, b) = (norm(datei), norm(kandidat));
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 100.0;
    }
    if b.ends_with(&a) || a.ends_with(&b) {
        return 90.0; // Herstellerpraefix davor („<Hersteller> 35mmF1.4 R")
    }
    // Brennweite und Lichtstaerke muessen stimmen — sonst ist es ein anderes Glas.
    let (Some(ea), Some(eb)) = (eckdaten(datei), eckdaten(kandidat)) else {
        return 0.0;
    };
    if (ea.0 - eb.0).abs() > 0.5 || (ea.1 - eb.1).abs() > 0.5 {
        return 0.0;
    }
    if ea.2 > 0.0 && eb.2 > 0.0 && (ea.2 - eb.2).abs() > 0.05 {
        return 0.0;
    }
    let wa = woerter(datei);
    let wb = woerter(kandidat);
    let gemeinsam = wa.iter().filter(|w| wb.contains(w)).count() as f32;
    let alle = (wa.len() + wb.len()) as f32 - gemeinsam;
    40.0 + 40.0 * gemeinsam / alle.max(1.0)
}

// ---------------------------------------------------------------------------
// lensfun
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Verz {
    PtLens(f32, f32, f32),
    Poly3(f32),
    Poly5(f32, f32),
}

#[derive(Debug, Clone)]
enum Tca {
    Linear(f32, f32),
    Poly3([f32; 3], [f32; 3]), // (v, c, b) rot, (v, c, b) blau
}

#[derive(Debug, Clone)]
pub struct DbObjektiv {
    pub hersteller: String,
    pub modelle: Vec<String>,
    pub anschluesse: Vec<String>,
    pub crop: f32,
    pub seitenverhaeltnis: f32,
    verz: Vec<(f32, Verz)>,
    tca: Vec<(f32, Tca)>,
    /// (Brennweite, Blende, Abstand, k1, k2, k3)
    vign: Vec<(f32, f32, f32, [f32; 3])>,
}

#[derive(Debug, Clone)]
pub struct DbKamera {
    pub hersteller: String,
    pub modell: String,
    pub anschluss: String,
    pub crop: f32,
}

#[derive(Debug, Default)]
pub struct Objektivdatenbank {
    pub objektive: Vec<DbObjektiv>,
    pub kameras: Vec<DbKamera>,
}

fn tag_inhalt<'a>(block: &'a str, tag: &str) -> Vec<&'a str> {
    let mut v = Vec::new();
    let mut rest = block;
    let auf = format!("<{tag}");
    let zu = format!("</{tag}>");
    while let Some(p) = rest.find(&auf) {
        let nach = &rest[p + auf.len()..];
        // Nur das echte Tag, nicht ein laengeres mit gleichem Anfang.
        if !nach.starts_with('>') && !nach.starts_with(' ') {
            rest = nach;
            continue;
        }
        let Some(gt) = nach.find('>') else { break };
        let innen = &nach[gt + 1..];
        let Some(e) = innen.find(&zu) else { break };
        v.push(innen[..e].trim());
        rest = &innen[e + zu.len()..];
    }
    v
}

fn attr(tag: &str, name: &str) -> Option<f32> {
    let muster = format!(" {name}=\"");
    let p = tag.find(&muster)?;
    let rest = &tag[p + muster.len()..];
    rest[..rest.find('"')?].trim().parse().ok()
}

fn attr_text<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let muster = format!(" {name}=\"");
    let p = tag.find(&muster)?;
    let rest = &tag[p + muster.len()..];
    Some(&rest[..rest.find('"')?])
}

/// Alle selbstschliessenden Tags `<name ... />` eines Blocks.
fn leere_tags<'a>(block: &'a str, name: &str) -> Vec<&'a str> {
    let mut v = Vec::new();
    let auf = format!("<{name} ");
    let mut rest = block;
    while let Some(p) = rest.find(&auf) {
        let nach = &rest[p..];
        let Some(e) = nach.find('>') else { break };
        v.push(&nach[..e + 1]);
        rest = &nach[e + 1..];
    }
    v
}

fn ohne_kommentare(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find("<!--") {
        o.push_str(&rest[..p]);
        match rest[p..].find("-->") {
            Some(e) => rest = &rest[p + e + 3..],
            None => {
                rest = "";
                break;
            }
        }
    }
    o.push_str(rest);
    o
}

fn seitenverhaeltnis(t: &str) -> f32 {
    if let Some((a, b)) = t.split_once(':') {
        if let (Ok(a), Ok(b)) = (a.trim().parse::<f32>(), b.trim().parse::<f32>()) {
            if b > 0.0 {
                return (a / b).max(b / a);
            }
        }
    }
    t.trim().parse().unwrap_or(1.5)
}

impl Objektivdatenbank {
    pub fn aus_xml(xml: &str) -> Self {
        let xml = ohne_kommentare(xml);
        let mut db = Objektivdatenbank::default();
        for k in tag_inhalt(&xml, "camera") {
            let hersteller = tag_inhalt(k, "maker").first().copied().unwrap_or("").to_string();
            let modell = tag_inhalt(k, "model").first().copied().unwrap_or("").to_string();
            let anschluss = tag_inhalt(k, "mount").first().copied().unwrap_or("").to_string();
            let crop = tag_inhalt(k, "cropfactor").first().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            db.kameras.push(DbKamera { hersteller, modell, anschluss, crop });
        }
        for l in tag_inhalt(&xml, "lens") {
            let hersteller = tag_inhalt(l, "maker").first().copied().unwrap_or("").to_string();
            let modelle: Vec<String> = tag_inhalt(l, "model").iter().map(|s| s.to_string()).collect();
            let anschluesse: Vec<String> = tag_inhalt(l, "mount").iter().map(|s| s.to_string()).collect();
            let crop = tag_inhalt(l, "cropfactor").first().and_then(|v| v.parse().ok()).unwrap_or(1.0);
            let seiten = tag_inhalt(l, "aspect-ratio").first().map(|v| seitenverhaeltnis(v)).unwrap_or(1.5);
            let mut o = DbObjektiv {
                hersteller,
                modelle,
                anschluesse,
                crop,
                seitenverhaeltnis: seiten,
                verz: Vec::new(),
                tca: Vec::new(),
                vign: Vec::new(),
            };
            for t in leere_tags(l, "distortion") {
                let f = attr(t, "focal").unwrap_or(0.0);
                let modell = attr_text(t, "model").unwrap_or("");
                let v = match modell {
                    "ptlens" => Verz::PtLens(attr(t, "a").unwrap_or(0.0), attr(t, "b").unwrap_or(0.0), attr(t, "c").unwrap_or(0.0)),
                    "poly3" => Verz::Poly3(attr(t, "k1").unwrap_or(0.0)),
                    "poly5" => Verz::Poly5(attr(t, "k1").unwrap_or(0.0), attr(t, "k2").unwrap_or(0.0)),
                    _ => continue,
                };
                o.verz.push((f, v));
            }
            for t in leere_tags(l, "tca") {
                let f = attr(t, "focal").unwrap_or(0.0);
                let v = match attr_text(t, "model").unwrap_or("") {
                    "linear" => Tca::Linear(attr(t, "kr").unwrap_or(1.0), attr(t, "kb").unwrap_or(1.0)),
                    "poly3" => Tca::Poly3(
                        [attr(t, "vr").unwrap_or(1.0), attr(t, "cr").unwrap_or(0.0), attr(t, "br").unwrap_or(0.0)],
                        [attr(t, "vb").unwrap_or(1.0), attr(t, "cb").unwrap_or(0.0), attr(t, "bb").unwrap_or(0.0)],
                    ),
                    _ => continue,
                };
                o.tca.push((f, v));
            }
            for t in leere_tags(l, "vignetting") {
                if attr_text(t, "model") != Some("pa") {
                    continue;
                }
                o.vign.push((
                    attr(t, "focal").unwrap_or(0.0),
                    attr(t, "aperture").unwrap_or(0.0),
                    attr(t, "distance").unwrap_or(1000.0),
                    [attr(t, "k1").unwrap_or(0.0), attr(t, "k2").unwrap_or(0.0), attr(t, "k3").unwrap_or(0.0)],
                ));
            }
            o.verz.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            o.tca.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if !o.verz.is_empty() || !o.tca.is_empty() || !o.vign.is_empty() {
                db.objektive.push(o);
            }
        }
        db
    }

    /// Die mitgelieferte Datenbank (einmal entpackt und gelesen).
    pub fn mitgeliefert() -> &'static Objektivdatenbank {
        static DB: OnceLock<Objektivdatenbank> = OnceLock::new();
        DB.get_or_init(|| {
            use std::io::Read;
            let roh: &[u8] = include_bytes!("../daten/lensfun.xml.z");
            let mut xml = String::new();
            let mut d = flate2::read::ZlibDecoder::new(roh);
            if d.read_to_string(&mut xml).is_err() {
                return Objektivdatenbank::default();
            }
            Objektivdatenbank::aus_xml(&xml)
        })
    }

    pub fn kamera(&self, hersteller: &str, modell: &str) -> Option<&DbKamera> {
        let (h, m) = (norm(hersteller), norm(modell));
        self.kameras
            .iter()
            .find(|k| norm(&k.modell) == m && (h.contains(&norm(&k.hersteller)) || norm(&k.hersteller).contains(&h)))
    }

    /// Das passende Objektiv: Name (unscharf) + Anschluss der Kamera.
    pub fn objektiv(&self, kamera: Option<&DbKamera>, objektiv: &str) -> Option<&DbObjektiv> {
        let mut bestes: Option<(&DbObjektiv, f32)> = None;
        for o in &self.objektive {
            let mut p = 0f32;
            for m in &o.modelle {
                p = p.max(passung(objektiv, m)).max(passung(objektiv, &format!("{} {}", o.hersteller, m)));
            }
            if p <= 0.0 {
                continue;
            }
            if let Some(k) = kamera {
                if !o.anschluesse.is_empty() && !o.anschluesse.iter().any(|a| norm(a) == norm(&k.anschluss)) {
                    // Anderer Anschluss: nur bei exaktem Namen (Adapter) gelten lassen.
                    if p < 90.0 {
                        continue;
                    }
                    p -= 5.0;
                }
            }
            if bestes.map(|(_, b)| p > b).unwrap_or(true) {
                bestes = Some((o, p));
            }
        }
        // Unter 60 Punkten ist der Name zu vage — eine falsche Korrektur
        // waere schlimmer als keine.
        bestes.filter(|(_, p)| *p >= 60.0).map(|(o, _)| o)
    }
}

fn lerp_brennweite<T: Clone>(eintraege: &[(f32, T)], f: f32, mischen: impl Fn(&T, &T, f32) -> T) -> Option<T> {
    if eintraege.is_empty() {
        return None;
    }
    if f <= eintraege[0].0 {
        return Some(eintraege[0].1.clone());
    }
    for w in eintraege.windows(2) {
        if f <= w[1].0 {
            let t = (f - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
            return Some(mischen(&w[0].1, &w[1].1, t));
        }
    }
    Some(eintraege[eintraege.len() - 1].1.clone())
}

impl DbObjektiv {
    /// Die Korrektur fuer eine Aufnahme. `crop` = Formatfaktor der Kamera.
    pub fn radial(&self, brennweite: f32, blende: f32, crop: f32, breite: u32, hoehe: u32) -> Option<Radial> {
        let crop_k = if self.crop > 0.0 { self.crop } else { crop };
        let crop_f = if crop > 0.0 { crop } else { crop_k };
        let asp_f = (breite.max(hoehe) as f32 / breite.min(hoehe).max(1) as f32).max(1.0);
        let diag_mm = (36f32.hypot(24.0)) / 2.0;
        // Halbe Diagonale des Fotos in mm; Hugin-Einheit (halbe kurze Seite)
        // und Ecken-Einheit des Kalibrier-Sensors in mm.
        let halbdiag_foto = diag_mm / crop_f;
        let halbkurz_kal = diag_mm / crop_k / self.seitenverhaeltnis.hypot(1.0) * 1.0;
        let halbdiag_kal = diag_mm / crop_k;
        let _ = asp_f;
        let zu_hugin = halbdiag_foto / halbkurz_kal;
        let zu_ecke = halbdiag_foto / halbdiag_kal;

        let verz = lerp_brennweite(&self.verz, brennweite, |a, b, t| match (a, b) {
            (Verz::PtLens(a0, b0, c0), Verz::PtLens(a1, b1, c1)) => {
                Verz::PtLens(a0 + (a1 - a0) * t, b0 + (b1 - b0) * t, c0 + (c1 - c0) * t)
            }
            (Verz::Poly3(k0), Verz::Poly3(k1)) => Verz::Poly3(k0 + (k1 - k0) * t),
            (Verz::Poly5(a0, b0), Verz::Poly5(a1, b1)) => Verz::Poly5(a0 + (a1 - a0) * t, b0 + (b1 - b0) * t),
            _ => if t < 0.5 { a.clone() } else { b.clone() },
        });
        let tca = lerp_brennweite(&self.tca, brennweite, |a, b, t| match (a, b) {
            (Tca::Linear(r0, b0), Tca::Linear(r1, b1)) => Tca::Linear(r0 + (r1 - r0) * t, b0 + (b1 - b0) * t),
            (Tca::Poly3(r0, b0), Tca::Poly3(r1, b1)) => {
                let m = |x: &[f32; 3], y: &[f32; 3]| [x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t];
                Tca::Poly3(m(r0, r1), m(b0, b1))
            }
            _ => if t < 0.5 { a.clone() } else { b.clone() },
        });
        let vign = self.vignette_bei(brennweite, blende);

        let m = |rz: f32| -> f32 {
            let r = rz * zu_hugin;
            match &verz {
                Some(Verz::PtLens(a, b, c)) => a * r * r * r + b * r * r + c * r + 1.0 - a - b - c,
                Some(Verz::Poly3(k)) => 1.0 - k + k * r * r,
                Some(Verz::Poly5(k1, k2)) => 1.0 + k1 * r * r + k2 * r * r * r * r,
                None => 1.0,
            }
        };
        let ca = |rq: f32, rot: bool| -> f32 {
            let r = rq * zu_hugin;
            match &tca {
                Some(Tca::Linear(kr, kb)) => (if rot { *kr } else { *kb }) - 1.0,
                Some(Tca::Poly3(rr, bb)) => {
                    let k = if rot { rr } else { bb };
                    k[2] * r * r + k[1] * r + k[0] - 1.0
                }
                None => 0.0,
            }
        };
        let gain = |rq: f32| -> f32 {
            match vign {
                Some([k1, k2, k3]) => {
                    let r2 = (rq * zu_ecke).powi(2);
                    1.0 / (1.0 + k1 * r2 + k2 * r2 * r2 + k3 * r2 * r2 * r2).max(0.2)
                }
                None => 1.0,
            }
        };
        if verz.is_none() && tca.is_none() && vign.is_none() {
            return None;
        }
        radial_aus(breite, hoehe, &m, &|r| ca(r, true), &|r| ca(r, false), &gain)
    }

    /// Vignettierung bei Brennweite/Blende (groesster Abstand = Unendlich).
    fn vignette_bei(&self, f: f32, n: f32) -> Option<[f32; 3]> {
        if self.vign.is_empty() {
            return None;
        }
        let abstand = self.vign.iter().map(|v| v.2).fold(0.0f32, f32::max);
        let mut brennweiten: Vec<f32> = self.vign.iter().filter(|v| v.2 == abstand).map(|v| v.0).collect();
        brennweiten.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        brennweiten.dedup();
        let bei_brennweite = |fw: f32| -> Option<[f32; 3]> {
            let mut zeilen: Vec<(f32, [f32; 3])> = self
                .vign
                .iter()
                .filter(|v| v.2 == abstand && (v.0 - fw).abs() < 1e-3)
                .map(|v| (v.1, v.3))
                .collect();
            zeilen.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            // Blende unbekannt → f/5.6 annehmen. Interpoliert in log(Blende).
            let ziel = if n > 0.0 { n } else { 5.6 }.ln();
            let mit_ln: Vec<(f32, [f32; 3])> = zeilen.iter().map(|(b, k)| (b.max(0.5).ln(), *k)).collect();
            lerp_brennweite(&mit_ln, ziel, |a, b, t| {
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
            })
        };
        let liste: Vec<(f32, [f32; 3])> = brennweiten.iter().filter_map(|fw| bei_brennweite(*fw).map(|k| (*fw, k))).collect();
        lerp_brennweite(&liste, f, |a, b, t| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t])
    }
}

// ---------------------------------------------------------------------------
// Installierte Objektivprofile (LCP, von anderen Anwendungen mitgebracht)
// ---------------------------------------------------------------------------

fn lcp_orte() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(eigen) = std::env::var("FOTOWACHE_LCP") {
        v.push(PathBuf::from(eigen));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        for p in [
            "/Applications/Adobe Lightroom.app/Contents/Resources/LensProfiles/1.0", // Kennung: Suchpfad
            "/Applications/Adobe Lightroom Classic/Adobe Lightroom Classic.app/Contents/Resources/LensProfiles/1.0", // Kennung: Suchpfad
            "/Library/Application Support/Adobe/CameraRaw/LensProfiles/1.0", // Kennung: Suchpfad
        ] {
            v.push(PathBuf::from(p));
        }
        if let Some(h) = &home {
            v.push(h.join("Library/Application Support/Adobe/CameraRaw/LensProfiles/1.0")); // Kennung: Suchpfad
        }
    } else if cfg!(target_os = "windows") {
        v.push(PathBuf::from(r"C:\ProgramData\Adobe\CameraRaw\LensProfiles\1.0")); // Kennung: Suchpfad
        if let Some(a) = std::env::var_os("APPDATA") {
            v.push(PathBuf::from(a).join(r"Adobe\CameraRaw\LensProfiles\1.0")); // Kennung: Suchpfad
        }
    }
    v
}

/// Alle LCP-Dateien (einmal je Programmlauf gesucht).
fn lcp_dateien() -> &'static Vec<PathBuf> {
    static LISTE: OnceLock<Vec<PathBuf>> = OnceLock::new();
    LISTE.get_or_init(|| {
        let mut v = Vec::new();
        fn sammeln(dir: &Path, v: &mut Vec<PathBuf>, tiefe: u32) {
            if tiefe > 6 {
                return;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    sammeln(&p, v, tiefe + 1);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lcp")) {
                    v.push(p);
                }
            }
        }
        if std::env::var_os("FOTOWACHE_OHNE_FREMDPROFILE").is_none() {
            for o in lcp_orte() {
                sammeln(&o, &mut v, 0);
            }
        }
        v
    })
}

#[derive(Debug, Clone, Default)]
struct LcpEintrag {
    brennweite: f32,
    blende_av: f32,
    abstand: f32,
    roh: bool,
    fx: f32,
    verz: [f32; 3],
    ca_r: Option<([f32; 3], f32)>,
    ca_b: Option<([f32; 3], f32)>,
    vign: Option<[f32; 3]>,
    vign_stuecke: Vec<(f32, f32)>,
}

fn lcp_wert(block: &str, name: &str) -> Option<f32> {
    attr(&format!(" {}", block), &format!("stCamera:{name}"))
}

fn lcp_lesen(text: &str, objektiv: &str) -> Vec<LcpEintrag> {
    let mut v = Vec::new();
    for li in tag_inhalt(text, "rdf:li") {
        // Nur Eintraege, deren Objektiv zum Foto passt.
        let mit_rand = format!(" {li}");
        let lens = attr_text(&mit_rand, "stCamera:Lens").unwrap_or("");
        let schoen = attr_text(&mit_rand, "stCamera:LensPrettyName").unwrap_or("");
        if passung(objektiv, lens).max(passung(objektiv, schoen)) < 60.0 {
            continue;
        }
        let Some(f) = lcp_wert(li, "FocalLength") else { continue };
        let mut e = LcpEintrag {
            brennweite: f,
            blende_av: lcp_wert(li, "ApertureValue").unwrap_or(0.0),
            abstand: lcp_wert(li, "FocusDistance").unwrap_or(1000.0),
            roh: li.contains("stCamera:CameraRawProfile=\"True\""), // Kennung: Feldname des LCP-Formats
            fx: lcp_wert(li, "FocalLengthX").unwrap_or(0.0),
            ..Default::default()
        };
        // Verzeichnung: das erste RadialDistortParam (PerspectiveModel).
        if let Some(p) = li.find("PerspectiveModel") {
            let pm = &li[p..];
            e.verz = [
                lcp_wert(pm, "RadialDistortParam1").unwrap_or(0.0),
                lcp_wert(pm, "RadialDistortParam2").unwrap_or(0.0),
                lcp_wert(pm, "RadialDistortParam3").unwrap_or(0.0),
            ];
            if e.fx == 0.0 {
                e.fx = lcp_wert(pm, "FocalLengthX").unwrap_or(0.0);
            }
        }
        for (name, ziel) in [("ChromaticRedGreenModel", 0), ("ChromaticBlueGreenModel", 1)] {
            if let Some(p) = li.find(name) {
                let rest = &li[p..];
                let ende = rest.find("/>").or_else(|| rest.find("</stCamera:")).unwrap_or(rest.len());
                let blk = &rest[..ende];
                if let Some(k1) = lcp_wert(blk, "RadialDistortParam1") {
                    let k = [k1, lcp_wert(blk, "RadialDistortParam2").unwrap_or(0.0), lcp_wert(blk, "RadialDistortParam3").unwrap_or(0.0)];
                    let s = lcp_wert(blk, "ScaleFactor").unwrap_or(1.0);
                    if ziel == 0 {
                        e.ca_r = Some((k, s));
                    } else {
                        e.ca_b = Some((k, s));
                    }
                }
            }
        }
        if let Some(p) = li.find("VignetteModel") {
            let vm = &li[p..];
            if let Some(a1) = lcp_wert(vm, "VignetteModelParam1") {
                e.vign = Some([a1, lcp_wert(vm, "VignetteModelParam2").unwrap_or(0.0), lcp_wert(vm, "VignetteModelParam3").unwrap_or(0.0)]);
            }
            if let Some(q) = vm.find("VignetteModelPiecewiseParam") {
                for wert in tag_inhalt(&vm[q..], "rdf:li").iter().take(64) {
                    if let Some((r, g)) = wert.split_once(',') {
                        if let (Ok(r), Ok(g)) = (r.trim().parse::<f32>(), g.trim().parse::<f32>()) {
                            e.vign_stuecke.push((r, g));
                        }
                    }
                }
            }
        }
        v.push(e);
    }
    v
}

/// Installiertes Objektivprofil (LCP) fuer Kamera + Objektiv, falls eine andere
/// Anwendung eines mitgebracht hat.
pub fn lcp_radial(
    kamera_hersteller: &str,
    objektiv: &str,
    brennweite: f32,
    blende: f32,
    brennweite_kb: f32,
    breite: u32,
    hoehe: u32,
) -> Option<Radial> {
    if objektiv.trim().is_empty() {
        return None;
    }
    let (f0, f1, _) = eckdaten(objektiv)?;
    let bereich = if (f1 - f0).abs() < 0.1 {
        format!("{}mm", f0 as u32)
    } else {
        format!("{}-{}mm", f0 as u32, f1 as u32)
    };
    let hersteller = norm(kamera_hersteller.split_whitespace().next().unwrap_or(""));
    let mut alle: Vec<LcpEintrag> = Vec::new();
    for pfad in lcp_dateien() {
        let name = pfad.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !name.replace(' ', "").contains(&bereich) {
            continue;
        }
        // Profile sind je Kamerasystem abgelegt (Ordner „<HERSTELLER> (…)").
        if !hersteller.is_empty() && !norm(&name).starts_with(&hersteller) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(pfad) {
            alle.extend(lcp_lesen(&text, objektiv));
        }
        if alle.len() > 400 {
            break;
        }
    }
    if alle.is_empty() {
        return None;
    }
    // RAW-Profile bevorzugen (JPEG-Profile tragen schon Kamera-Korrekturen).
    if alle.iter().any(|e| e.roh) {
        alle.retain(|e| e.roh);
    }
    // Grosser Abstand (Unendlich) als Normalfall; Blende am naechsten.
    let abstand = alle.iter().map(|e| e.abstand).fold(0.0f32, f32::max);
    alle.retain(|e| (e.abstand - abstand).abs() < 1e-3 || abstand == 0.0);
    let av = if blende > 0.0 { 2.0 * blende.log2() } else { 5.0 };
    let mut brennweiten: Vec<f32> = alle.iter().map(|e| e.brennweite).collect();
    brennweiten.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    brennweiten.dedup_by(|a, b| (*a - *b).abs() < 0.05);
    let waehle = |fw: f32| -> Option<LcpEintrag> {
        alle.iter()
            .filter(|e| (e.brennweite - fw).abs() < 0.05)
            .min_by(|a, b| (a.blende_av - av).abs().partial_cmp(&(b.blende_av - av).abs()).unwrap_or(std::cmp::Ordering::Equal))
            .cloned()
    };
    // Naechstgelegene Brennweite (die Referenz-Entwicklung mischt zwischen Stuetzstellen —
    // die Profile sind dicht genug, dass die naechste genuegt).
    let fw = *brennweiten
        .iter()
        .min_by(|a, b| (*a - brennweite).abs().partial_cmp(&(*b - brennweite).abs()).unwrap_or(std::cmp::Ordering::Equal))?;
    let e = waehle(fw)?;
    // Brennweite in Einheiten der laengeren Bildseite.
    let fx_norm = if e.fx > 0.0 {
        e.fx
    } else {
        let kb = if brennweite_kb > 0.0 { brennweite_kb } else { brennweite * 1.5 };
        kb / 36.0
    };
    let dmax = breite.max(hoehe) as f32;
    let halbdiag = ((breite as f32).hypot(hoehe as f32)) / 2.0;
    let zu_f = halbdiag / (fx_norm * dmax); // Diagonal-Anteil → Brennweiten-Einheiten
    let poly = |k: &[f32; 3], r: f32| {
        let r2 = r * r;
        1.0 + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2
    };
    let m = |rz: f32| poly(&e.verz, rz * zu_f);
    let ca = |rq: f32, c: &Option<([f32; 3], f32)>| -> f32 {
        match c {
            Some((k, s)) => s * poly(k, rq * zu_f) - 1.0,
            None => 0.0,
        }
    };
    let gain = |rq: f32| -> f32 {
        let r = rq * zu_f;
        if e.vign_stuecke.len() >= 2 {
            let k: Vec<f32> = e.vign_stuecke.iter().map(|p| p.0).collect();
            let w: Vec<f32> = e.vign_stuecke.iter().map(|p| p.1).collect();
            let mut v = w[w.len() - 1];
            if r <= k[0] {
                v = w[0];
            } else {
                for i in 1..k.len() {
                    if r <= k[i] {
                        let t = (r - k[i - 1]) / (k[i] - k[i - 1]).max(1e-9);
                        v = w[i - 1] + (w[i] - w[i - 1]) * t;
                        break;
                    }
                }
            }
            1.0 / v.max(0.2)
        } else if let Some(a) = &e.vign {
            1.0 / poly(a, r).max(0.2)
        } else {
            1.0
        }
    };
    radial_aus(breite, hoehe, &m, &|r| ca(r, &e.ca_r), &|r| ca(r, &e.ca_b), &gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eckdaten_aus_namen() {
        assert_eq!(eckdaten("18-300mm F/3.5-6.3 DiIII-A VC VXD B061X"), Some((18.0, 300.0, 3.5)));
        assert_eq!(eckdaten("XF35mmF1.4 R"), Some((35.0, 35.0, 1.4))); // Kennung: echter Objektivname
        assert_eq!(eckdaten("RF24-105mm F4 L IS USM"), Some((24.0, 105.0, 4.0)));
    }

    #[test]
    fn namen_passen_unscharf() {
        assert!(passung("18-300mm F/3.5-6.3 DiIII-A VC VXD B061X", "18-300mm F3.5-6.3 DiIII-A VC VXD B061X") >= 100.0);
        assert!(passung("XF35mmF1.4 R", "Fujifilm XF35mmF1.4 R") >= 90.0); // Kennung: echte Objektivnamen
        // Andere Lichtstaerke = anderes Objektiv.
        assert_eq!(passung("XF35mmF2 R WR", "XF35mmF1.4 R"), 0.0); // Kennung: echte Objektivnamen
    }

    #[test]
    fn mitgelieferte_datenbank_kennt_das_reisezoom() {
        let db = Objektivdatenbank::mitgeliefert();
        assert!(db.objektive.len() > 1000, "{} Objektive", db.objektive.len());
        let kamera = db.kamera("Fujifilm", "X-T50").or_else(|| db.kamera("Fujifilm", "X-T30")); // Kennung: Datenbankeintraege
        let o = db.objektiv(kamera, "18-300mm F/3.5-6.3 DiIII-A VC VXD B061X").expect("Reisezoom B061");
        assert!(o.anschluesse.iter().any(|a| a.contains("Fujifilm"))); // Kennung: Anschlussname der Datenbank
        let r = o.radial(18.0, 5.6, 1.534, 7728, 5152).expect("Korrektur");
        // Bei 18 mm: Tonne → am Rand holt die Korrektur von weiter innen.
        assert!(r.m_an(1.0) < 1.0, "{}", r.m_an(1.0));
    }
}
