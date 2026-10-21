//! Sprache der Oberflaeche: Deutsch (Quellsprache) und Englisch.
//!
//! Jeder sichtbare Text steht im Code auf Deutsch und geht durch `t()` bzw.
//! `tf()`. Bei Englisch kommt die Uebersetzung aus der festen Tabelle in
//! `sprache_en.rs`; fehlt dort ein Eintrag, bleibt der deutsche Text stehen —
//! lieber deutsch als leer. Die Oberflaeche holt ihre Texte in jedem Bild neu,
//! deshalb wirkt ein Umschalten sofort.
//!
//! Welche Sprache gilt, in dieser Reihenfolge:
//! 1. die Umgebungsvariable `FOTOWACHE_SPRACHE` (`de` / `en`),
//! 2. die gespeicherte Wahl (Datei `sprache.txt` im Einstellungsordner, siehe
//!    `ordner::einstellungen`),
//! 3. die Systemsprache: beginnt sie mit „de", Deutsch, sonst Englisch.
//!
//! Nicht hier hindurch geht, was an den KI-Server geschickt wird (`ai.rs`): der
//! Wortlaut der Anweisung ist ein Vertrag mit dem Server.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[path = "sprache_en.rs"]
mod en;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sprache {
    De,
    En,
}

impl Sprache {
    /// Name in der eigenen Sprache — fuer die Auswahl, deshalb nicht uebersetzt.
    pub fn name(self) -> &'static str {
        match self {
            Sprache::De => "Deutsch",
            Sprache::En => "English",
        }
    }

    fn kuerzel(self) -> &'static str {
        match self {
            Sprache::De => "de",
            Sprache::En => "en",
        }
    }

    /// „de", „DE", „de_DE.UTF-8" → Deutsch; „en", „en-US" → Englisch; sonst nichts.
    fn aus_kuerzel(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        if s.starts_with("de") {
            Some(Sprache::De)
        } else if s.starts_with("en") {
            Some(Sprache::En)
        } else {
            None
        }
    }
}

/// Die gerade gueltige Sprache: 0 = noch nicht ermittelt, 1 = Deutsch,
/// 2 = Englisch. Ein Atom statt eines Schlosses — `t()` laeuft auch in den
/// Arbeitsfaeden (KI-Protokoll, Stapel) und in jedem Bild hundertfach.
static AKTUELL: AtomicU8 = AtomicU8::new(0);

/// Name der Datei im Einstellungsordner, die die Wahl aufhebt.
const DATEI: &str = "sprache.txt";

pub fn aktuell() -> Sprache {
    match AKTUELL.load(Ordering::Relaxed) {
        1 => Sprache::De,
        2 => Sprache::En,
        _ => {
            let s = ermitteln();
            merken(s);
            s
        }
    }
}

fn merken(s: Sprache) {
    let wert = match s {
        Sprache::De => 1,
        Sprache::En => 2,
    };
    AKTUELL.store(wert, Ordering::Relaxed);
}

/// Umschalten aus der Oberflaeche: gilt sofort und beim naechsten Start.
pub fn setzen(s: Sprache) {
    merken(s);
    if let Some(ordner) = crate::ordner::einstellungen() {
        // Scheitert das Schreiben, gilt die Wahl eben nur bis zum Beenden —
        // kein Grund, die Arbeit zu unterbrechen.
        let _ = std::fs::create_dir_all(&ordner);
        let _ = std::fs::write(ordner.join(DATEI), s.kuerzel());
    }
}

fn ermitteln() -> Sprache {
    let umgebung = std::env::var("FOTOWACHE_SPRACHE").ok();
    let gespeichert = crate::ordner::einstellungen()
        .and_then(|ordner| std::fs::read_to_string(ordner.join(DATEI)).ok());
    let system = sys_locale::get_locale();
    waehlen(umgebung.as_deref(), gespeichert.as_deref(), system.as_deref())
}

/// Die Rangfolge an einer Stelle — getrennt von Umgebung und Platte, damit sie
/// sich pruefen laesst. Unbrauchbare Werte („xx") zaehlen wie nicht gesetzt.
fn waehlen(umgebung: Option<&str>, gespeichert: Option<&str>, system: Option<&str>) -> Sprache {
    umgebung
        .and_then(Sprache::aus_kuerzel)
        .or_else(|| gespeichert.and_then(Sprache::aus_kuerzel))
        .unwrap_or(match system {
            Some(s) if s.trim().to_ascii_lowercase().starts_with("de") => Sprache::De,
            _ => Sprache::En,
        })
}

/// Die Tabelle als Nachschlagewerk — einmal aufgebaut, danach nur gelesen.
fn tabelle() -> &'static HashMap<&'static str, &'static str> {
    static TABELLE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    TABELLE.get_or_init(|| en::EN.iter().copied().collect())
}

/// Ein Text in der aktuellen Sprache. `de` ist der deutsche Text, wie er im
/// Quelltext steht; er ist zugleich der Schluessel in die Tabelle.
pub fn t(de: &'static str) -> &'static str {
    t_in(aktuell(), de)
}

fn t_in(s: Sprache, de: &'static str) -> &'static str {
    match s {
        Sprache::De => de,
        Sprache::En => tabelle().get(de).copied().unwrap_or(de),
    }
}

/// Ein Text mit Werten: die `{}` der (uebersetzten) Vorlage werden der Reihe
/// nach durch `werte` ersetzt. Formatierte Zahlen („{:.1}") vorher selbst in
/// Text verwandeln — die Vorlage kennt nur das schlichte `{}`.
pub fn tf(de: &'static str, werte: &[&dyn std::fmt::Display]) -> String {
    einsetzen(t(de), werte)
}

fn einsetzen(vorlage: &str, werte: &[&dyn std::fmt::Display]) -> String {
    let mut aus = String::with_capacity(vorlage.len() + 8 * werte.len());
    let mut rest = vorlage;
    let mut i = 0;
    while let Some(p) = rest.find("{}") {
        aus.push_str(&rest[..p]);
        match werte.get(i) {
            Some(w) => {
                let _ = write!(aus, "{w}");
            }
            // Fehlt ein Wert, bleibt die Luecke sichtbar stehen, statt still
            // zu verschwinden.
            None => aus.push_str("{}"),
        }
        i += 1;
        rest = &rest[p + 2..];
    }
    aus.push_str(rest);
    aus
}

/// Fuer Texte, die erst zur Laufzeit feststehen, aber aus einer bekannten Menge
/// kommen (z. B. Profilnamen aus der Farbkette): uebersetzt, wenn die Tabelle
/// sie kennt, sonst unveraendert (Namen installierter Kameraprofile (DCP) sind
/// schon englisch).
pub fn t_laufzeit(text: &str) -> &str {
    match aktuell() {
        Sprache::De => text,
        Sprache::En => tabelle().get(text).copied().unwrap_or(text),
    }
}

// ---------------------------------------------------------------------------
// Texte der anderen Crates
//
// `rawcore` und `pipeline` bringen ihre Meldungen fest auf Deutsch mit. Sie
// bleiben dort unangetastet; die Oberflaeche setzt die Texte hier selbst
// zusammen, Variante fuer Variante.
// ---------------------------------------------------------------------------

/// Fehler beim Lesen einer RAW-Datei.
pub fn raw_fehler(e: &rawcore::RawError) -> String {
    use rawcore::RawError as E;
    match e {
        E::Path(pfad) => tf("Datei nicht lesbar: {}", &[pfad]),
        E::Open(m) => tf("LibRaw konnte die Datei nicht öffnen: {}", &[m]),
        E::Unpack(m) => tf("LibRaw konnte die Rohdaten nicht entpacken: {}", &[m]),
        E::Process(m) => tf("LibRaw konnte das Bild nicht entwickeln: {}", &[m]),
        E::NoImage(code) => tf("LibRaw lieferte kein Bild zurück (Fehler {})", &[code]),
        E::Unexpected { bits, colors } => {
            tf("unerwartetes Bildformat von LibRaw: {} Bit, {} Kanäle", &[bits, colors])
        }
        E::OutOfMemory => t("LibRaw ist ohne Speicher").to_string(),
    }
}

/// Fehler der Grafikkarte.
pub fn gpu_fehler(e: &pipeline::GpuError) -> String {
    use pipeline::GpuError as E;
    match e {
        E::NoAdapter => t("keine geeignete Grafikkarte gefunden").to_string(),
        E::Device(m) => tf("Grafikkarte abgelehnt: {}", &[m]),
        E::Empty => t("Bild ist leer").to_string(),
    }
}

/// Wie der Bildschirm in der Fusszeile beschrieben wird — wie
/// `pipeline::Gpu::monitor_text`, aber uebersetzbar.
pub fn monitor_text(gpu: &pipeline::Gpu) -> String {
    match &gpu.monitor {
        Some(p) => tf(
            "{} · {} % sRGB-Fläche",
            &[&p.name, &format!("{:.0}", p.gamut_vs_srgb * 100.0)],
        ),
        None => t("Bildschirm: sRGB angenommen").to_string(),
    }
}

/// Eine Zahl mit `stellen` Nachkommastellen im Format der Oberflaeche
/// (Deutsch mit Komma).
pub fn zahl(v: f32, stellen: usize) -> String {
    let s = format!("{v:.stellen$}");
    match aktuell() {
        Sprache::De => s.replace('.', ","),
        Sprache::En => s,
    }
}

/// Fortschrittsschritt des Panorama-Kerns.
pub fn panorama_schritt(s: pipeline::panorama::Schritt) -> String {
    use pipeline::panorama::Schritt as S;
    match s {
        S::Merkmale => t("Merkmale suchen"),
        S::Zuordnen => t("Fotos zuordnen"),
        S::Ausrichten => t("Ausrichten"),
        S::Feinausrichtung => t("Feinausrichtung"),
        S::Parallaxe => t("Parallaxe ausgleichen"),
        S::Naehte => t("Nähte suchen"),
        S::Mischen => t("Mischen"),
        S::Fertig => t("Fertig"),
    }
    .to_string()
}

/// Warum ein Panorama nicht entstehen konnte.
pub fn panorama_fehler(e: &pipeline::panorama::PanoramaFehler) -> String {
    use pipeline::panorama::PanoramaFehler as E;
    match e {
        E::ZuWenigBilder => t("Für ein Panorama mindestens zwei Fotos auswählen.").to_string(),
        E::UngueltigeAbmessungen(k) => tf("Foto {} hat ungültige Abmessungen.", &[&(k + 1)]),
        E::KeineUeberlappung => t("Keine zwei Fotos überlappen erkennbar — ein Panorama braucht etwa 20–50 % Überlappung zwischen Nachbarfotos.").to_string(),
        E::NichtAusrichtbar => t("Die Fotos ließen sich nicht widerspruchsfrei ausrichten.").to_string(),
        E::ProjektionUngeeignet => t("Die Projektion kann diese Anordnung nicht darstellen — bitte Zylinder oder Kugel wählen.").to_string(),
        E::AusserhalbProjektion => t("Ein Foto liegt außerhalb der darstellbaren Fläche — bitte eine andere Projektion wählen.").to_string(),
    }
}

/// Der Bericht des Panorama-Kerns, eine Zeile je Meldung.
pub fn panorama_bericht(bericht: &[pipeline::panorama::Meldung]) -> String {
    use pipeline::panorama::{Meldung as M, Projektion as P, Verwerfgrund as G};
    let zeile = |m: &M| -> String {
        match m {
            M::Verwendet { genutzt, von } => tf("Panorama aus {} von {} Fotos.", &[genutzt, von]),
            M::Verworfen { bild, grund } => {
                let g = match grund {
                    G::KeinPartner { hoechstens_punkte: 0 } => t("passt zu keinem anderen Foto (keine übereinstimmenden Punkte)").to_string(),
                    G::KeinPartner { hoechstens_punkte } => {
                        tf("passt zu keinem anderen Foto (höchstens {} übereinstimmende Punkte)", &[hoechstens_punkte])
                    }
                    G::ZweiteGruppe => t("gehört zu einer zweiten, getrennten Gruppe").to_string(),
                    G::LageWiderspruechlich => t("Lage widerspricht den übrigen Fotos").to_string(),
                };
                tf("Weggelassen: Foto {} – {}.", &[&(bild + 1), &g])
            }
            M::Ausrichtung { fehler_px, punktpaare, prozent_nachgefuehrt } => tf(
                "Mittlerer Ausrichtungsfehler: {} px ({} Punktpaare, {} % auf voller Auflösung nachgeführt).",
                &[&zahl(*fehler_px, 2), punktpaare, prozent_nachgefuehrt],
            ),
            M::Brennweite { px, aus_metadaten } => tf(
                "Brennweite: {} px ({}).",
                &[&zahl(*px, 0), &if *aus_metadaten { t("aus den Metadaten, nachgeregelt") } else { t("geschätzt") }],
            ),
            M::BrennweiteFestgelegt => t("Hinweis: Die Fotos passen nicht zu einem festen Standpunkt (geschätzte Brennweite unplausibel) — Brennweite aus den Metadaten festgehalten, Versatz möglich.").to_string(),
            M::Massstab { bild, faktor, groesser: true } => tf(
                "Hinweis: Foto {} erscheint {}-fach größer als die übrigen (anderer Zoom oder anderer Standort).",
                &[&(bild + 1), &zahl(*faktor, 2)],
            ),
            M::Massstab { bild, faktor, groesser: false } => tf(
                "Hinweis: Foto {} erscheint {}-fach kleiner als die übrigen (anderer Zoom oder anderer Standort).",
                &[&(bild + 1), &zahl(*faktor, 2)],
            ),
            M::Verzeichnung { tonne: true, prozent } => {
                tf("Objektivverzeichnung ausgeglichen (Tonne, {} % in den Bildecken).", &[&zahl(*prozent, 1)])
            }
            M::Verzeichnung { tonne: false, prozent } => {
                tf("Objektivverzeichnung ausgeglichen (Kissen, {} % in den Bildecken).", &[&zahl(*prozent, 1)])
            }
            M::Belichtung { von, bis } => tf("Helligkeit angeglichen: Verstärkung {} bis {}.", &[&zahl(*von, 2), &zahl(*bis, 2)]),
            M::Parallaxe { vorher_px, nachher_px, .. } => tf(
                "Parallaxe ausgeglichen: Versatz an den Nähten {} px statt {} px.",
                &[&zahl(*nachher_px, 1), &zahl(*vorher_px, 1)],
            ),
            M::Projektion { projektion, w, h, verkleinert_prozent } => {
                let name = match projektion {
                    P::Zylinder => t("Zylinder"),
                    P::Kugel => t("Kugel"),
                    P::Eben => t("Eben"),
                };
                match verkleinert_prozent {
                    Some(p) => tf("Projektion: {}, {} × {} px (auf {} % verkleinert).", &[&name, w, h, &zahl(*p, 0)]),
                    None => tf("Projektion: {}, {} × {} px.", &[&name, w, h]),
                }
            }
            M::Beschnitten => {
                t("Hinweis: Der Bildwinkel übersteigt, was diese Projektion darstellen kann; die Ränder sind abgeschnitten.").to_string()
            }
        }
    };
    bericht.iter().map(zeile).collect::<Vec<_>>().join("\n")
}

/// Namen der Kameraprofile, die die Farbkette selbst vergibt (alle anderen
/// kommen aus installierten Kameraprofilen (DCP) oder DNG-Dateien und bleiben,
/// wie sie sind).
/// Steht hier, damit die Pruefung sie kennt.
#[cfg(test)]
pub(crate) const PROFILNAMEN: &[&str] = &["Eingebettet", "Kameraprofil"];

#[cfg(test)]
pub(crate) mod pruefung {
    //! Werkzeug fuer die Tests: findet die Texte der Oberflaeche im Quelltext.

    /// Alle Quelldateien der Oberflaeche (ohne die Tabelle selbst).
    pub(crate) const QUELLEN: &[(&str, &str)] = &[
        ("ai.rs", include_str!("ai.rs")),
        ("export.rs", include_str!("export.rs")),
        ("generator.rs", include_str!("generator.rs")),
        ("grundpanel.rs", include_str!("grundpanel.rs")),
        ("ki_lokal.rs", include_str!("ki_lokal.rs")),
        ("main.rs", include_str!("main.rs")),
        ("ordner.rs", include_str!("ordner.rs")),
        ("panorama.rs", include_str!("panorama.rs")),
        ("presets.rs", include_str!("presets.rs")),
        ("sprache.rs", include_str!("sprache.rs")),
        ("updater.rs", include_str!("updater.rs")),
        ("zusatz.rs", include_str!("zusatz.rs")),
    ];

    /// Die englische Tabelle, fuer die Schriftzeichen-Pruefung in `main.rs`.
    pub(crate) fn tabelle_en() -> &'static [(&'static str, &'static str)] {
        super::en::EN
    }

    /// Der Quelltext ohne Kommentarzeilen und ohne Testmodule — gezeigt wird
    /// nur, was im Programm selbst steht. Die Zeilen bleiben an ihrem Platz
    /// (ausgeblendete werden leer), damit Meldungen die richtige Nummer nennen.
    pub(crate) fn nur_programm(quelle: &str) -> String {
        let mut aus = String::with_capacity(quelle.len());
        let mut test_folgt = false;
        let mut im_test = false;
        for zeile in quelle.lines() {
            let getrimmt = zeile.trim_start();
            let behalten = if im_test {
                // Testmodule stehen ganz links und enden mit einer Zeile, die
                // nur aus „}" besteht (so formatiert rustfmt).
                if zeile == "}" {
                    im_test = false;
                }
                false
            } else if zeile.trim() == "#[cfg(test)]" {
                test_folgt = true;
                false
            } else if test_folgt
                && !zeile.starts_with(char::is_whitespace)
                && zeile.contains("mod ")
                && zeile.trim_end().ends_with('{')
            {
                test_folgt = false;
                im_test = true;
                false
            } else {
                test_folgt = false;
                !getrimmt.starts_with("//")
            };
            if behalten {
                aus.push_str(zeile);
            }
            aus.push('\n');
        }
        aus
    }

    /// Ein Text aus `t("…")` oder `tf("…")` im Quelltext.
    pub(crate) struct Fund {
        pub zeile: usize,
        pub mit_werten: bool,
        pub text: String,
    }

    /// Findet alle `t("…")` und `tf("…")` und liefert ihre Texte so, wie sie
    /// zur Laufzeit ankommen (Escapes und Zeilenfortsetzungen aufgeloest). Das
    /// Literal darf auch erst auf der naechsten Zeile beginnen, wie rustfmt es
    /// bei langen Aufrufen setzt.
    pub(crate) fn texte(quelle: &str) -> Vec<Fund> {
        let code = nur_programm(quelle);
        let mut funde = Vec::new();
        for (pos, _) in code.match_indices('t') {
            // Nur ein eigenstaendiger Name: „ist(" oder „text(" zaehlen nicht.
            let davor = code[..pos].chars().next_back();
            if davor.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let rest = &code[pos..];
            let (mit_werten, klammer) = if rest.starts_with("t(") {
                (false, 2)
            } else if rest.starts_with("tf(") {
                (true, 3)
            } else {
                continue;
            };
            let argument = rest[klammer..].trim_start();
            if !argument.starts_with('"') {
                continue;
            }
            let Some(text) = literal_lesen(argument) else {
                continue;
            };
            let zeile = code[..pos].matches('\n').count() + 1;
            funde.push(Fund { zeile, mit_werten, text });
        }
        funde
    }

    /// Liest ein Rust-Zeichenkettenliteral ab dem oeffnenden `"`.
    fn literal_lesen(s: &str) -> Option<String> {
        let mut aus = String::new();
        let mut zeichen = s.chars().skip(1).peekable();
        while let Some(c) = zeichen.next() {
            match c {
                '"' => return Some(aus),
                '\\' => match zeichen.next()? {
                    'n' => aus.push('\n'),
                    't' => aus.push('\t'),
                    'r' => aus.push('\r'),
                    '0' => aus.push('\0'),
                    '\\' => aus.push('\\'),
                    '"' => aus.push('"'),
                    '\'' => aus.push('\''),
                    // Zeilenfortsetzung: Umbruch und Einrueckung entfallen.
                    '\n' => {
                        while zeichen.peek().is_some_and(|w| w.is_whitespace()) {
                            zeichen.next();
                        }
                    }
                    'u' => {
                        let mut hex = String::new();
                        for h in zeichen.by_ref() {
                            match h {
                                '{' => {}
                                '}' => break,
                                h => hex.push(h),
                            }
                        }
                        aus.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                    }
                    _ => return None,
                },
                c => aus.push(c),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::pruefung::{texte, QUELLEN};
    use super::*;
    use std::collections::HashSet;

    /// Texte, die nicht als Literal in `t("…")` stehen, sondern aus Tabellen
    /// oder anderen Crates kommen und erst bei der Anzeige uebersetzt werden.
    fn laufzeit_texte() -> Vec<&'static str> {
        let mut v: Vec<&'static str> = Vec::new();
        v.extend(crate::presets::PRESETS.iter().map(|p| p.name));
        v.extend(crate::presets::FILMMODI.iter().map(|m| m.name));
        v.extend(crate::presets::FILMMODI.iter().map(|m| m.hinweis));
        v.extend(crate::grundpanel::WB_VORGABEN.iter().map(|(n, _, _)| *n));
        use pipeline::objektiv::{Objektiv, ObjektivQuelle as Q};
        for quelle in [
            Q::Datei,
            Q::Kamera,
            Q::Installiert,
            Q::Datenbank,
            Q::Profil,
            Q::Modul,
            Q::Brennweite,
            Q::Keine,
        ] {
            v.push(Objektiv { quelle, ..Default::default() }.quelle_text());
        }
        v.extend(PROFILNAMEN);
        v.extend(crate::grundpanel::LOOK_NAMEN);
        v
    }

    fn platzhalter(s: &str) -> usize {
        s.matches("{}").count()
    }

    /// (a) Jeder Text der Oberflaeche hat eine englische Fassung.
    #[test]
    fn jeder_text_hat_eine_uebersetzung() {
        let mut fehlt = Vec::new();
        for (datei, quelle) in QUELLEN {
            for f in texte(quelle) {
                if !tabelle().contains_key(f.text.as_str()) {
                    fehlt.push(format!("{datei}:{}: {:?}", f.zeile, f.text));
                }
            }
        }
        for text in laufzeit_texte() {
            if !tabelle().contains_key(text) {
                fehlt.push(format!("(Tabelle/Crate): {text:?}"));
            }
        }
        assert!(fehlt.is_empty(), "ohne englische Fassung:\n{}", fehlt.join("\n"));
    }

    /// Die Suche selbst muss funktionieren — sonst waere die Pruefung oben
    /// leer und damit immer gruen.
    #[test]
    fn die_textsuche_findet_die_texte() {
        let alle: Vec<_> = QUELLEN.iter().flat_map(|(_, q)| texte(q)).collect();
        assert!(alle.len() > 200, "nur {} Texte gefunden", alle.len());
        let probe = "fn f() {\n    // t(\"Kommentar\")\n    ui.label(t(\"Eins\"));\n    \
                     let x = tf(\"Zwei {}\\n\\\n         weiter\", &[&1]);\n    ist(\"nein\");\n    \
                     y(t(\n        \"Drei\",\n    ));\n    fn t(de: &str) {}\n}\n";
        let funde = texte(probe);
        let gefunden: Vec<_> = funde.iter().map(|f| (f.zeile, f.mit_werten, f.text.as_str())).collect();
        assert_eq!(
            gefunden,
            vec![(3, false, "Eins"), (4, true, "Zwei {}\nweiter"), (7, false, "Drei")]
        );
    }

    /// (b) Gleich viele Platzhalter in beiden Sprachen, und nur schlichte `{}`.
    #[test]
    fn platzhalter_stimmen_ueberein() {
        let mut falsch = Vec::new();
        for (de, en) in en::EN {
            if platzhalter(de) != platzhalter(en) {
                falsch.push(format!("{de:?} → {en:?}: {} statt {}", platzhalter(en), platzhalter(de)));
            }
            for s in [de, en] {
                let ohne = s.replace("{}", "");
                if ohne.contains('{') || ohne.contains('}') {
                    falsch.push(format!("Klammer ausser {{}} in {s:?}"));
                }
            }
        }
        // t() setzt nichts ein — ein „{}" darin stuende roh auf dem Schirm;
        // tf() ohne Platzhalter ist ein Versehen.
        for (datei, quelle) in QUELLEN {
            for f in texte(quelle) {
                if f.mit_werten != (platzhalter(&f.text) > 0) {
                    falsch.push(format!("{datei}:{}: {:?}", f.zeile, f.text));
                }
            }
        }
        assert!(falsch.is_empty(), "{}", falsch.join("\n"));
    }

    /// (c) Kein deutscher Text steht zweimal in der Tabelle — der zweite
    /// Eintrag wuerde still gewinnen.
    #[test]
    fn keine_doppelten_schluessel() {
        let mut gesehen = HashSet::new();
        let doppelt: Vec<_> = en::EN.iter().filter(|(de, _)| !gesehen.insert(*de)).map(|(de, _)| *de).collect();
        assert!(doppelt.is_empty(), "doppelt: {doppelt:?}");
        assert!(en::EN.iter().all(|(de, en)| !de.is_empty() && !en.is_empty()));
    }

    /// Kein Eintrag ohne Verwendung: ein vertippter Schluessel saehe aus wie
    /// eine Uebersetzung, griffe aber nie.
    #[test]
    fn jeder_eintrag_wird_gebraucht() {
        let mut benutzt: HashSet<String> = QUELLEN
            .iter()
            .flat_map(|(_, q)| texte(q))
            .map(|f| f.text)
            .collect();
        benutzt.extend(laufzeit_texte().into_iter().map(String::from));
        let ungenutzt: Vec<_> = en::EN.iter().filter(|(de, _)| !benutzt.contains(*de)).map(|(de, _)| *de).collect();
        assert!(ungenutzt.is_empty(), "ungenutzt: {ungenutzt:?}");
    }

    /// Reine Testmodule ohne Oberflaechentexte.
    const NUR_TESTS: &[&str] = &[];

    /// Jede Quelldatei der Oberflaeche wird geprueft — auch eine neue.
    #[test]
    fn alle_quelldateien_werden_geprueft() {
        let ordner = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut dateien: Vec<String> = std::fs::read_dir(ordner)
            .expect("src lesbar")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".rs") && n != "sprache_en.rs" && !NUR_TESTS.contains(&n.as_str()))
            .collect();
        dateien.sort();
        let geprueft: Vec<&str> = QUELLEN.iter().map(|(n, _)| *n).collect();
        assert_eq!(dateien, geprueft);
    }

    #[test]
    fn uebersetzen_und_einsetzen() {
        assert_eq!(t_in(Sprache::De, "Belichtung"), "Belichtung");
        assert_eq!(t_in(Sprache::En, "Belichtung"), "Exposure");
        // Fehlt ein Eintrag, bleibt es deutsch — nie leer.
        assert_eq!(t_in(Sprache::En, "Gibt es nicht"), "Gibt es nicht");
        assert_eq!(einsetzen(t_in(Sprache::En, "{} Fotos · {} bearbeitet"), &[&12, &3]), "12 photos · 3 edited");
        assert_eq!(einsetzen("{} von {}", &[&1]), "1 von {}");
        assert_eq!(einsetzen("ohne", &[]), "ohne");
    }

    #[test]
    fn sprachwahl_in_der_richtigen_rangfolge() {
        // Umgebung vor gespeicherter Wahl vor System.
        assert_eq!(waehlen(Some("en"), Some("de"), Some("de-DE")), Sprache::En);
        assert_eq!(waehlen(Some("de"), Some("en"), Some("en-US")), Sprache::De);
        assert_eq!(waehlen(None, Some("en\n"), Some("de-DE")), Sprache::En);
        assert_eq!(waehlen(None, Some("de"), Some("en-US")), Sprache::De);
        // Unbrauchbare Werte zaehlen nicht.
        assert_eq!(waehlen(Some("xx"), Some(""), Some("de_AT")), Sprache::De);
        // System: „de…" heisst Deutsch, alles andere Englisch.
        assert_eq!(waehlen(None, None, Some("de-CH")), Sprache::De);
        assert_eq!(waehlen(None, None, Some("en-GB")), Sprache::En);
        assert_eq!(waehlen(None, None, Some("fr-FR")), Sprache::En);
        assert_eq!(waehlen(None, None, None), Sprache::En);
    }
}
