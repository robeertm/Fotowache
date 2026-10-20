//! Vorlagen — 1:1 aus der Web-Version uebernommen.
//!
//! Bewusst dieselben Namen und Werte: wer die Web-Version kennt, findet sie
//! wieder, und ein „Porträt" muss auf dem Rechner dasselbe tun wie im Browser.
//! Uebernommen aus `site/app.js` (`PRESETS`).
//!
//! Namen und Hinweise stehen hier deutsch und werden erst bei der Anzeige
//! uebersetzt (`sprache::t`) — die Filmlooks genauso wie die Vorlagen.

use pipeline::params::Edit;

pub struct Preset {
    /// Deutscher Name; angezeigt wird `t(name)`.
    pub name: &'static str,
    pub apply: fn(&mut Edit),
}

pub struct Filmmodus {
    /// Deutscher Name; angezeigt wird `t(name)`. Er wird nirgends gespeichert
    /// (ein Look setzt nur Reglerwerte) — ein neuer Name bricht keine
    /// Begleitdatei.
    pub name: &'static str,
    /// Kurzbeschreibung fuers Schweben mit der Maus — wofuer der Modus gedacht
    /// ist. Deutsch; angezeigt wird `t(hinweis)`.
    pub hinweis: &'static str,
    pub apply: fn(&mut Edit),
}

/// Die Filmlooks: die eingebauten Bildmodi der RAF-Kameras, auf unsere Regler
/// uebersetzt.
///
/// Das sind NACHEMPFINDUNGEN, keine Kopien — die Originale in der Kamera sind
/// 3D-Farbtabellen. Der Massstab: „Ausgewogen" ist die Standardentwicklung der
/// Testkamera, auf die auch die Grundwerte (Grundwaerme, Grundschaerfung,
/// Zielhelligkeit) kalibriert sind. Alle anderen Looks beschreiben ihren ABSTAND
/// dazu, so wie der Kamerahersteller seine Bildmodi relativ zum Standard
/// beschreibt (gesaettigter, weicher, entsaettigt-dokumentarisch …).
pub const FILMMODI: &[Filmmodus] = &[
    Filmmodus {
        name: "Ausgewogen",
        hinweis: "Standard — ausgewogen, wie die Kamera selbst entwickelt",
        apply: |e| {
            e.contrast += 6.0;
            e.vibrance += 12.0;
            e.saturation += 4.0;
        },
    },
    Filmmodus {
        name: "Leuchtend",
        hinweis: "Diafilm-Look — kräftige Farben, tiefe Schatten. Landschaft.",
        apply: |e| {
            e.contrast += 18.0;
            e.vibrance += 26.0;
            e.saturation += 14.0;
            e.blacks += -8.0;
            e.clarity += 6.0;
            e.hsl_s = [0.0, 0.0, 0.0, 8.0, 6.0, 10.0, 0.0, 0.0];
        },
    },
    Filmmodus {
        name: "Sanft",
        hinweis: "Weiche Kontraste, zarte Haut — Porträt im Freien",
        apply: |e| {
            e.contrast += -6.0;
            e.highlights += -12.0;
            e.shadows += 10.0;
            e.vibrance += 12.0;
            e.clarity += -6.0;
        },
    },
    Filmmodus {
        name: "Natürlich",
        hinweis: "Naturgetreue Farben mit sattem Kontrast — Alltag, Reportage",
        apply: |e| {
            e.contrast += 10.0;
            e.vibrance += 8.0;
            e.saturation += 5.0;
            e.highlights += -6.0;
        },
    },
    Filmmodus {
        name: "Gedämpft",
        hinweis: "Gedeckte Farben, harte Schatten — Magazin-Reportage",
        apply: |e| {
            e.saturation += -18.0;
            e.vibrance += -6.0;
            e.contrast += 14.0;
            e.blacks += -6.0;
            e.shadows += -6.0;
            e.temp += -4.0; // etwas kuehler als die Aufnahme
            e.hsl_s = [-10.0, 0.0, 0.0, 0.0, 0.0, -14.0, 0.0, 0.0];
        },
    },
    Filmmodus {
        name: "Retro-Negativ",
        hinweis: "Farbnegativfilm — kontrastreich, grünliche Schatten, Alltagspoesie",
        apply: |e| {
            e.saturation += -14.0;
            e.contrast += 18.0;
            e.grade_shadow_hue = 170.0;
            e.grade_shadow_sat = 8.0;
            e.grade_high_hue = 40.0;
            e.grade_high_sat = 8.0;
            e.grain = 12.0;
        },
    },
    Filmmodus {
        name: "Warmes Negativ",
        hinweis: "Bernsteinlicht, offene Schatten — 70er-Farbfilm",
        apply: |e| {
            e.temp += 8.0;
            e.shadows += 14.0;
            e.contrast += 8.0;
            e.saturation += -8.0;
            e.grade_high_hue = 45.0;
            e.grade_high_sat = 10.0;
        },
    },
    Filmmodus {
        name: "Porträt-Negativ",
        hinweis: "Sehr weich, neutrale Haut — Studio-Porträt",
        apply: |e| {
            e.contrast += -10.0;
            e.saturation += -6.0;
            e.highlights += -8.0;
            e.shadows += 6.0;
        },
    },
    Filmmodus {
        name: "Filmisch",
        hinweis: "Kinofilm — flach, entsättigt, viel Spielraum",
        apply: |e| {
            e.contrast += -18.0;
            e.saturation += -22.0;
            e.highlights += -12.0;
            e.shadows += 12.0;
            e.blacks += 8.0;
        },
    },
    Filmmodus {
        name: "Silberglanz",
        hinweis: "Silberhaltig entwickelt — hart, fast farblos, dramatisch",
        apply: |e| {
            e.saturation += -35.0;
            e.contrast += 25.0;
            e.clarity += 10.0;
            e.blacks += -10.0;
        },
    },
    Filmmodus {
        name: "Feines Schwarzweiß",
        hinweis: "Feinkörniger S/W-Film — dichte Tiefen, feines Korn",
        apply: |e| {
            e.bw = true;
            e.contrast += 14.0;
            e.clarity += 8.0;
            e.grain = 12.0;
        },
    },
];

pub const PRESETS: &[Preset] = &[
    Preset {
        name: "Original",
        // Setzt den LOOK auf Null — aber wirft nicht die Aufnahme-Grundlagen
        // weg: Kameralage/Zuschnitt/Retusche sind keine Optik-Entscheidung,
        // und Grundhelligkeit + ISO-Rauschminderung gehoeren zur Aufnahme
        // (ohne sie lag das Foto 2-3 Blenden dunkel und voller Farbgesprenkel
        // des 6x6-Farbmusters — sichtbar als Uebersprechen einzelner Pixel beim
        // Entwickeln von Hand ab „Original").
        apply: |_| {
            // Die Grundausstattung des Fotos (Automatik bzw. XMP-Einstellungen)
            // ist bereits die Basis jeder Vorlage — „Original" fuegt nichts
            // hinzu. Aufnahme-Grundlagen (Drehung, Zuschnitt, Retusche,
            // Objektiv) bleiben ohnehin erhalten.
        },
    },
    Preset {
        name: "Kräftig",
        apply: |e| {
            e.contrast += 22.0;
            e.vibrance += 35.0;
            e.saturation += 8.0;
            e.clarity += 15.0;
            e.sharpen += -8.0;
        },
    },
    Preset {
        name: "Porträt",
        apply: |e| {
            e.exposure += 8.0;
            e.contrast += 8.0;
            e.highlights += -15.0;
            e.shadows += 18.0;
            e.vibrance += 18.0;
            e.clarity += -8.0;
            e.temp += -4.0;
            e.sharpen += -18.0;
        },
    },
    Preset {
        name: "Landschaft",
        apply: |e| {
            e.contrast += 18.0;
            e.vibrance += 30.0;
            e.saturation += 6.0;
            e.clarity += 22.0;
            e.dehaze += 15.0;
            e.sharpen += -3.0;
            e.hsl_s = [0.0, 0.0, 0.0, 25.0, 10.0, 15.0, 0.0, 0.0];
        },
    },
    Preset {
        name: "Warm",
        apply: |e| {
            e.temp += 23.0;
            e.tint += 8.0;
            e.vibrance += 15.0;
            e.contrast += 8.0;
        },
    },
    Preset {
        name: "Kühl",
        apply: |e| {
            e.temp += -47.0;
            e.tint += -6.0;
            e.vibrance += 12.0;
            e.contrast += 8.0;
        },
    },
    Preset {
        name: "Matt",
        apply: |e| {
            e.contrast += -18.0;
            e.blacks += 22.0;
            e.highlights += -10.0;
            e.vibrance += -8.0;
            e.saturation += -6.0;
            e.curve = [0.08, 0.28, 0.5, 0.74, 0.94];
        },
    },
    Preset {
        name: "Vintage",
        apply: |e| {
            e.temp += 8.0;
            e.tint += 12.0;
            e.contrast += -8.0;
            e.vibrance += -10.0;
            e.saturation += -12.0;
            e.blacks += 15.0;
            e.vignette = 30.0;
            e.grain = 25.0;
            e.curve = [0.06, 0.27, 0.5, 0.72, 0.9];
        },
    },
    Preset {
        name: "Kino",
        apply: |e| {
            e.contrast += 15.0;
            e.temp += -22.0;
            e.shadows += 12.0;
            e.highlights += -12.0;
            e.vibrance += 12.0;
            e.hsl_h = [0.0, 0.0, 0.0, 0.0, -8.0, 8.0, 0.0, 0.0];
            e.vignette = 18.0;
            e.saturation += -4.0;
        },
    },
    Preset {
        name: "S/W Klassik",
        apply: |e| {
            e.bw = true;
            e.contrast += 15.0;
            e.clarity += 10.0;
            e.sharpen += -13.0;
        },
    },
    Preset {
        name: "S/W Hart",
        apply: |e| {
            e.bw = true;
            e.contrast += 40.0;
            e.clarity += 22.0;
            e.blacks += -15.0;
            e.whites += 15.0;
            e.sharpen += -8.0;
            e.grain = 15.0;
        },
    },
];

/// Grundeinstellung fuer ein frisch geoeffnetes RAW.
///
/// Ein lineares RAW sieht ohne Kurve flau und dunkel aus — das ist kein Fehler,
/// sondern Physik: der Sensor misst Licht, das Auge sieht logarithmisch. Kameras
/// und gaengige Raw-Entwickler legen deshalb immer eine Grundkurve an. Wir tun
/// dasselbe, sonst saehe jedes Bild beim Oeffnen kaputt aus.
pub fn start_edit() -> Edit {
    Edit {
        // Neutrale Nutzerkurve: die Grundkurve steckt im Kameraprofil.
        curve: [0.0, 0.25, 0.5, 0.75, 1.0],
        // Standard der Referenz-Entwicklung fuer RAW: Schaerfen 40 / Radius 1,0 /
        // Detail 25 / Maskierung 0, Farbrauschen 25 — genau das steht in ihren
        // XMP-Dateien.
        sharpen: 40.0,
        sharpen_radius: 0.0,
        sharpen_mask: 0.0,
        color_noise: 25.0,
        prozess: pipeline::params::PROZESS,
        ..Edit::default()
    }
}

/// Die Grundausstattung von Prozess 1 (bis Fotowache 1.4) — nur noch noetig, um
/// bei alten Begleitdateien zu erkennen, ob jemand wirklich etwas verstellt hat.
pub fn start_edit_v1() -> Edit {
    Edit {
        curve: [0.0, 0.25, 0.5, 0.75, 1.0],
        sharpen: 38.0,
        sharpen_radius: 0.0,
        vibrance: 44.0,
        saturation: 18.0,
        sharpen_mask: 18.0,
        color_noise: 22.0,
        temp: 12.0,
        prozess: 1,
        ..Edit::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Filmlooks setzen einen LOOK — sie duerfen die Aufnahme-Grundausstattung
    /// (Grundschaerfung, Rauschglaettung) nicht anruehren. Die KI hat genau das
    /// einmal getan, und es fiel erst beim Nachmessen auf.
    #[test]
    fn bildmodi_ruehren_die_grundausstattung_nicht_an() {
        let start = start_edit();
        for m in FILMMODI {
            let mut e = start_edit();
            (m.apply)(&mut e);
            assert_eq!(e.sharpen, start.sharpen, "{} senkt die Grundschaerfung", m.name);
            assert_eq!(e.sharpen_mask, start.sharpen_mask, "{}", m.name);
            assert_eq!(e.color_noise, start.color_noise, "{}", m.name);
            assert_eq!(e.base_ev, start.base_ev, "{}", m.name);
        }
    }

    /// Jeder Look muss sich vom Standard unterscheiden — ein Knopf, der nichts
    /// tut, waere schlimmer als keiner.
    #[test]
    fn bildmodi_sind_unterscheidbar() {
        let mut gesehen: Vec<String> = Vec::new();
        for m in FILMMODI {
            let mut e = start_edit();
            (m.apply)(&mut e);
            let key = format!("{e:?}");
            assert!(!gesehen.contains(&key), "{} gleicht einem anderen Look", m.name);
            gesehen.push(key);
        }
    }

    /// Die Grundausstattung folgt dem RAW-Standard der Referenz-Entwicklung —
    /// ohne Grundwaerme: der Weissabgleich der Aufnahme gilt („Wie Aufnahme").
    #[test]
    fn grundausstattung_wie_referenz() {
        let e = start_edit();
        assert_eq!((e.sharpen, e.sharpen_radius, e.sharpen_mask), (40.0, 0.0, 0.0));
        assert_eq!(e.color_noise, 25.0);
        assert_eq!(e.temp, 0.0);
        assert_eq!(e.prozess, pipeline::params::PROZESS);
    }
}
