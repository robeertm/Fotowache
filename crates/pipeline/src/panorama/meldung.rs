//! Was der Panorama-Kern meldet: Fortschrittsschritte, Fehler und Bericht als
//! DATEN, nicht als fertige Saetze.
//!
//! Warum: die Oberflaeche ist mehrsprachig. Ein fertiger deutscher Satz aus dem
//! Kern laesst sich nicht uebersetzen — eine Variante mit Zahlen schon. Die
//! Saetze setzt die Oberflaeche in ihrer Sprache zusammen; die deutsche
//! Darstellung hier (`Display`, `bericht_deutsch`) dient dem Beispielprogramm,
//! den Pruefungen und dem Protokoll.

use super::Projektion;
use std::fmt;

/// Fortschrittsschritte in der Reihenfolge, in der sie laufen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Schritt {
    Merkmale,
    Zuordnen,
    Ausrichten,
    Feinausrichtung,
    Parallaxe,
    Naehte,
    Mischen,
    Fertig,
}

impl fmt::Display for Schritt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Schritt::Merkmale => "Merkmale suchen",
            Schritt::Zuordnen => "Bilder zuordnen",
            Schritt::Ausrichten => "Ausrichten",
            Schritt::Feinausrichtung => "Feinausrichtung",
            Schritt::Parallaxe => "Parallaxe ausgleichen",
            Schritt::Naehte => "Nähte suchen",
            Schritt::Mischen => "Mischen",
            Schritt::Fertig => "Fertig",
        })
    }
}

/// Warum ein Panorama nicht entstehen konnte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanoramaFehler {
    /// Weniger als zwei Bilder uebergeben.
    ZuWenigBilder,
    /// Bild mit diesem Index (0-basiert) hat unpassende Masse oder Daten.
    UngueltigeAbmessungen(usize),
    /// Keine zwei Bilder ueberlappen erkennbar.
    KeineUeberlappung,
    /// Die Bilder liessen sich nicht widerspruchsfrei ausrichten.
    NichtAusrichtbar,
    /// Die gewaehlte Projektion kann die Anordnung nicht darstellen.
    ProjektionUngeeignet,
    /// Ein Bild liegt ausserhalb der darstellbaren Flaeche.
    AusserhalbProjektion,
}

impl fmt::Display for PanoramaFehler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PanoramaFehler::ZuWenigBilder => f.write_str("Für ein Panorama braucht es mindestens zwei Bilder."),
            PanoramaFehler::UngueltigeAbmessungen(k) => write!(f, "Bild {} hat ungültige Abmessungen.", k + 1),
            PanoramaFehler::KeineUeberlappung => f.write_str(
                "Keine zwei Bilder überlappen erkennbar — ein Panorama braucht etwa 20–50 % Überlappung zwischen Nachbarbildern.",
            ),
            PanoramaFehler::NichtAusrichtbar => f.write_str("Die Bilder ließen sich nicht widerspruchsfrei ausrichten."),
            PanoramaFehler::ProjektionUngeeignet => {
                f.write_str("Die Projektion kann diese Bildanordnung nicht darstellen — bitte Zylinder oder Kugel wählen.")
            }
            PanoramaFehler::AusserhalbProjektion => {
                f.write_str("Ein Bild liegt außerhalb der darstellbaren Fläche — bitte eine andere Projektion wählen.")
            }
        }
    }
}

impl std::error::Error for PanoramaFehler {}

/// Warum ein Bild nicht verwendet wurde.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verwerfgrund {
    /// Passt zu keinem anderen Bild; `hoechstens_punkte` = beste Zahl
    /// uebereinstimmender Punkte mit irgendeinem Bild (0 = keine).
    KeinPartner { hoechstens_punkte: usize },
    /// Passt zu anderen Bildern, die aber eine zweite, getrennte Gruppe bilden.
    ZweiteGruppe,
    /// Seine Lage widerspricht den uebrigen Bildern.
    LageWiderspruechlich,
}

/// Eine Zeile des Berichts. Bildnummern sind Indizes der Eingabe (0-basiert),
/// Pixelangaben beziehen sich auf die Eingabebilder.
#[derive(Clone, Debug, PartialEq)]
pub enum Meldung {
    /// `genutzt` von `von` Bildern verwendet.
    Verwendet { genutzt: usize, von: usize },
    /// Bild `bild` wurde verworfen.
    Verworfen { bild: usize, grund: Verwerfgrund },
    /// Mittlerer Ausrichtungsfehler ueber `punktpaare` Punktpaare; davon
    /// `prozent_nachgefuehrt` % auf voller Aufloesung nachgefuehrt.
    Ausrichtung { fehler_px: f32, punktpaare: usize, prozent_nachgefuehrt: u32 },
    /// Ermittelte Brennweite; `aus_metadaten` = Startwert aus den Metadaten.
    Brennweite { px: f32, aus_metadaten: bool },
    /// Die geschaetzte Brennweite lag mehr als 30 % (3 σf nach Brown & Lowe)
    /// neben den Metadaten — die Aufnahmen passen nicht zu einer Drehung um
    /// einen festen Standpunkt (z. B. vom fahrenden Schiff). Die Brennweite der
    /// Metadaten wurde festgehalten; mit Versatz ist zu rechnen.
    BrennweiteFestgelegt,
    /// Bild `bild` erscheint `faktor`-fach (>= 1) groesser bzw. kleiner als die
    /// uebrigen (anderer Zoom oder anderer Standort).
    Massstab { bild: usize, faktor: f32, groesser: bool },
    /// Objektivverzeichnung ausgeglichen: Tonne (sonst Kissen), Betrag in den
    /// Bildecken in Prozent.
    Verzeichnung { tonne: bool, prozent: f32 },
    /// Belichtungsausgleich: Verstaerkungen von..bis.
    Belichtung { von: f32, bis: f32 },
    /// Parallaxe ausgeglichen: Restversatz an der schlechtesten Naht (Median
    /// und 90-%-Quantil, px) ohne und mit Ausgleich, groesste angewandte
    /// Verschiebung (px) und groesste Neigung, die der Ausgleich in eine Gerade
    /// bringt (Grad; Mass fuer die Verbiegung).
    Parallaxe { vorher_px: f32, nachher_px: f32, vorher_90_px: f32, nachher_90_px: f32, groesste_px: f32, groesste_neigung_grad: f32 },
    /// Projektion und Ausgabegroesse; `verkleinert_prozent` = Massstab der
    /// Ausgabe, wenn wegen der Groessengrenze verkleinert.
    Projektion { projektion: Projektion, w: u32, h: u32, verkleinert_prozent: Option<f32> },
    /// Der Bildwinkel uebersteigt die Projektion; Raender abgeschnitten.
    Beschnitten,
}

fn zahl(v: f64, stellen: usize) -> String {
    format!("{v:.stellen$}").replace('.', ",")
}

impl fmt::Display for Meldung {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Meldung::Verwendet { genutzt, von } => write!(f, "Panorama aus {genutzt} von {von} Bildern."),
            Meldung::Verworfen { bild, grund } => {
                let g = match grund {
                    Verwerfgrund::KeinPartner { hoechstens_punkte: 0 } => {
                        "passt zu keinem anderen Bild (keine übereinstimmenden Punkte)".to_string()
                    }
                    Verwerfgrund::KeinPartner { hoechstens_punkte } => {
                        format!("passt zu keinem anderen Bild (höchstens {hoechstens_punkte} übereinstimmende Punkte)")
                    }
                    Verwerfgrund::ZweiteGruppe => "gehört zu einer zweiten, getrennten Bildgruppe".to_string(),
                    Verwerfgrund::LageWiderspruechlich => "Lage widerspricht den übrigen Bildern".to_string(),
                };
                write!(f, "Verworfen: Bild {} – {g}.", bild + 1)
            }
            Meldung::Ausrichtung { fehler_px, punktpaare, prozent_nachgefuehrt } => write!(
                f,
                "Mittlerer Ausrichtungsfehler: {} px ({punktpaare} Punktpaare, {prozent_nachgefuehrt} % auf voller Auflösung nachgeführt).",
                zahl(*fehler_px as f64, 2)
            ),
            Meldung::Brennweite { px, aus_metadaten } => write!(
                f,
                "Brennweite: {} px ({}).",
                zahl(*px as f64, 0),
                if *aus_metadaten { "aus den Metadaten, nachgeregelt" } else { "geschätzt" }
            ),
            Meldung::BrennweiteFestgelegt => f.write_str(
                "Hinweis: Die Aufnahmen passen nicht zu einem festen Standpunkt (geschätzte Brennweite unplausibel) — Brennweite aus den Metadaten festgehalten, Versatz möglich.",
            ),
            Meldung::Massstab { bild, faktor, groesser } => write!(
                f,
                "Hinweis: Bild {} erscheint {}-fach {} als die übrigen (anderer Zoom oder anderer Standort).",
                bild + 1,
                zahl(*faktor as f64, 2),
                if *groesser { "größer" } else { "kleiner" }
            ),
            Meldung::Verzeichnung { tonne, prozent } => write!(
                f,
                "Objektivverzeichnung ausgeglichen ({} {} % in den Bildecken).",
                if *tonne { "Tonne" } else { "Kissen" },
                zahl(*prozent as f64, 1)
            ),
            Meldung::Belichtung { von, bis } => {
                write!(f, "Belichtungsausgleich: Verstärkung {} bis {}.", zahl(*von as f64, 2), zahl(*bis as f64, 2))
            }
            Meldung::Parallaxe { vorher_px, nachher_px, vorher_90_px, nachher_90_px, groesste_px, groesste_neigung_grad } => write!(
                f,
                "Parallaxe ausgeglichen: Restversatz an den Nähten {} px statt {} px (90 % unter {} px statt {} px; größte Verschiebung {} px, größte Neigung {}°).",
                zahl(*nachher_px as f64, 1),
                zahl(*vorher_px as f64, 1),
                zahl(*nachher_90_px as f64, 1),
                zahl(*vorher_90_px as f64, 1),
                zahl(*groesste_px as f64, 0),
                zahl(*groesste_neigung_grad as f64, 1)
            ),
            Meldung::Projektion { projektion, w, h, verkleinert_prozent } => {
                let name = match projektion {
                    Projektion::Zylinder => "Zylinder",
                    Projektion::Kugel => "Kugel",
                    Projektion::Eben => "Eben",
                };
                write!(f, "Projektion: {name}, {w} × {h} px")?;
                if let Some(p) = verkleinert_prozent {
                    write!(f, " (auf {} % verkleinert)", zahl(*p as f64, 0))?;
                }
                f.write_str(".")
            }
            Meldung::Beschnitten => {
                f.write_str("Hinweis: Der Bildwinkel übersteigt, was diese Projektion darstellen kann; die Ränder sind abgeschnitten.")
            }
        }
    }
}

/// Der Bericht als deutscher Text, eine Meldung je Zeile.
pub fn bericht_deutsch(bericht: &[Meldung]) -> String {
    bericht.iter().map(|m| m.to_string()).collect::<Vec<_>>().join("\n")
}
